# llm-harness-core 模块边界与分阶段可实现性审核

**日期：** 2026-06-07
**审核对象：** [2026-06-07-llm-harness-core-design.md](./2026-06-07-llm-harness-core-design.md) (v5)
**审核维度：** crate 间边界清晰度、依赖方向正确性、分阶段实现的可行性、各层独立可测试性

---

## 1. 依赖拓扑验证

### 1.1 声明依赖 vs 实际引用

**llm-harness-types** 声明依赖：`serde, serde_json, futures, tokio-util, thiserror, uuid, chrono`

逐类型检查 types crate 是否引用了声明外的 crate：

| 类型/函数 | 引用外部类型？ | 所属 crate |
|---|---|---|
| `EntryId` | `uuid::Uuid` | uuid (已声明) ✓ |
| `AgentMessage` | `chrono::DateTime<chrono::Utc>` | chrono (已声明) ✓ |
| `AgentEvent` | 全部字段为自有类型 + `std` | — ✓ |
| `Tool::execute` | `BoxFuture` | futures (已声明) ✓ |
| `ToolContext` | `CancellationToken`, `mpsc::Sender` | tokio-util + tokio (⚠️) |
| `ExecutionEnv` | `Path`, `PathBuf`, `Duration`, `BoxFuture`, `CancellationToken` | std + futures + tokio-util ✓ |
| `ShellOptions` | `Box<dyn FnMut>`, `Duration`, `CancellationToken` | std + tokio-util ✓ |
| 全部 Hook traits | `BoxFuture`, 自有类型 | futures + 自有 ✓ |
| `StreamOptions` | `serde_json::Value` | serde_json (已声明) ✓ |
| `TurnSnapshot` | `Arc<dyn Tool>` | 自有 ✓ |

> ⚠️ `ToolContext` (L283-291) 使用 `tokio::sync::mpsc::Sender<ToolResult>`。`tokio::sync::mpsc` 来自 `tokio` crate，但 types 的依赖列表中只有 `tokio-util`（不含 `tokio`）。需要将 `tokio` 加入 types 的依赖，或使用 `tokio-util` 重新导出的 channel 类型。实际上 `tokio-util` 不重导出 `tokio::sync::mpsc`，所以必须加 `tokio` 依赖（仅 `sync` feature）。

### 1.2 🔴 HarnessError 的 crate 边界违规

**涉及行：** L127-136 (types) vs L1159-1160 (harness)

```rust
// 定义在 llm-harness-types §3.1
pub enum HarnessError {
    #[error("harness is not idle (current phase: {0:?})")]
    NotIdle(crate::HarnessPhase),  // ← crate = llm_harness_types
    // ...
}
```

`crate::HarnessPhase` 在 types crate 上下文中解析为 `llm_harness_types::HarnessPhase`。但 `HarnessPhase` 的定义在 L1159-1160，属于 `llm-harness` crate（§5.6）：

```rust
// 定义在 llm-harness §5.6
pub enum HarnessPhase { Idle, Turning, Compacting, Branching }
```

**types crate 中没有 `HarnessPhase` 的定义。** 这段代码无法编译。

**修复方案（二选一）：**
- **方案 A：** 将 `HarnessPhase` 提升到 types crate（与 `AgentPhase` 并列）。`AgentPhase` 已在 L701 定义，将 `HarnessPhase` 也放入 types 的 §3.7 是合理的——它是两个 crate 都需要的共享类型。
- **方案 B：** 将 `HarnessError` 从 types 移到 harness crate。但这要求 harness 的所有方法返回 `HarnessError`（已在 harness 中定义），而 `SessionError`、`CompactionError` 等仍在 types 中——造成错误类型分散。

**推荐方案 A。** 同时也解决了 `not_idle` 方法在 Agent 和 AgentHarness 中各自返回不同错误类型的问题（Agent 用 `AgentError::NotIdle`，Harness 用 `HarnessError::NotIdle`——两者语义相同但类型不同，调用方需要分别处理）。

---

### 1.3 LlmClient / ModelInfo 的传递依赖

**涉及行：** L49-50（外部类型说明） vs L690, L1008（实际使用）

`ModelInfo` 来自 `llm-api-adapter`。使用位置：
- `AgentState.model_info: Option<ModelInfo>` (L690) — 在 harness crate
- `HarnessState.model_info: Option<ModelInfo>` (L1148) — 在 harness crate
- `CompactionSettings.summary_model_info: ModelInfo` (L1008) — 在 harness crate

依赖链路：`llm-harness` → `llm-harness-loop` → `llm-api-adapter`。`ModelInfo` 通过 `llm-harness-loop` 的**重导出**到达 `llm-harness`。设计文档未明确声明此重导出。

**建议：** 在 §4 开头明确写出 `llm-harness-loop` 的 `pub use` 项：

```rust
// llm-harness-loop 的 lib.rs
pub use llm_api_adapter::{LlmClient, ModelInfo};
```

否则 `llm-harness` 的 `Cargo.toml` 需要直接依赖 `llm-api-adapter`，破坏依赖图的层次性。

---

## 2. Crate 接口面分析

### 2.1 llm-harness-types 的公开接口面

| 类别 | 数量 | 关键项 |
|---|---|---|
| 标识/错误类型 | ~12 | EntryId, ToolError, AgentError, StopReason, EnvError, SessionError, CompactionError, TemplateError, HarnessError, DiagnosticLevel |
| 消息/内容类型 | ~10 | ContentBlock, ImageSource, AgentMessage (6 variants), TokenUsage |
| 事件类型 | 1 enum (16 variants) | AgentEvent |
| Trait | 2 核心 + 9 hook | Tool, ExecutionEnv, TransformContextHook, PrepareNextTurnHook, BeforeToolCallHook, AfterToolCallHook, ShouldStopHook, BeforeProviderRequestHook, AfterProviderResponseHook, AuthHook, BeforeTurnHook, AfterTurnHook, BeforeCompactHook |
| 配置/上下文 struct | ~8 | ToolContext, ToolResult, ShellOptions, ShellOutput, FileInfo, AgentContext, TurnSnapshot, StreamOptions |
| 枚举 | ~4 | ToolExecutionMode, ThinkingLevel, BeforeToolCallDecision, BeforeCompactDecision |

**评估：** ~45 个公开类型。对于一个 "纯类型层" 来说偏大，但考虑到这些类型确实被两个下游 crate 共享，这是合理的。唯一可削减的是 hook trait——如果某个 hook 只在 harness 层使用，可以移到 harness。当前所有 hook 都放 types 是保守但安全的做法。

**接口稳定性风险：** `AgentEvent` enum 的任何变体增加都是 breaking change（match 必须 exhaustive）。这在早期迭代中是预期的——v1 的 API 稳定性不在目标范围内。

### 2.2 llm-harness-loop 的公开接口面

| 类别 | 数量 | 关键项 |
|---|---|---|
| Trait | 2 | ConvertToLlmHook, CustomMessageConverter |
| Struct | 3 | DefaultConvertToLlm, LoopConfig, HookedTool |
| 函数 | 2 | agent_loop(), agent_loop_continue() |

**评估：** 仅 7 个公开项。**这是非常干净的接口**——loop 的复杂度完全内聚在 `agent_loop()` 的实现中。调用方只需要构造 `LoopConfig` + 提供 `LlmClient` + `AgentContext`，得到一个 `Stream`。接口面小意味着：
- 容易 mock 和测试
- API 稳定性高
- 替代实现（如 WebAssembly 版本的 loop）可行

### 2.3 llm-harness 的公开接口面

| 类别 | 数量 | 关键项 |
|---|---|---|
| Agent 相关 | ~4 | Agent, AgentState, AgentPhase, AgentOptions (隐式) |
| Session 相关 | ~20 | Session, SessionEntry, SessionEntryPayload (11 variants), SessionStorage trait, SessionRepo trait, SessionMetadata, BuiltContext, BranchInfo, JsonlSessionRepo, InMemorySessionRepo |
| Compaction 相关 | ~6 | CompactionSettings, CompactionPreparation, CompactionResult, FileOperation, prepare_compaction(), compact() |
| Skills/Templates | ~8 | Skill, SkillDiagnostic, SourcedSkill, PromptTemplate, load_skills(), format_skills_for_system_prompt(), load_prompt_templates(), invoke_template() |
| AgentHarness 相关 | ~10 | AgentHarness, HarnessState, HarnessPhase, HarnessHooks, AgentHarnessEvent (17 variants), CompactionStats |

**评估：** ~50 个公开项，分布在 5 个子系统中。**这个 crate 承担了太多职责。** 对于 v1 可以接受，但如果后续演进，建议拆分为：

```
llm-harness/              (AgentHarness, 依赖以下全部)
llm-harness-agent/        (Agent, AgentState)
llm-harness-session/      (Session, SessionStorage, Compaction)
llm-harness-resources/    (Skills, PromptTemplates)
```

但 v1 不拆分的理由充分：这些子系统之间有内部耦合（Compaction 依赖 SessionEntry，AgentHarness 依赖全部），拆分需要引入额外的 trait 抽象来解耦——这是过早抽象。

---

## 3. 分阶段实现路线图

### 阶段 1：`llm-harness-types`（可独立完成）

**依赖状态：** ✅ 全部依赖可用（serde, serde_json, futures, tokio-util, tokio, thiserror, uuid, chrono）

**可交付物：**
- 全部类型定义编译通过
- 类型上的 derive macros (Debug, Clone, Serialize, Deserialize, Error)
- `Tool` trait 和 `ExecutionEnv` trait（无实现）
- 全部 hook trait（无实现）

**可测试性：** ✅ 不需要任何 mock——全部是纯数据定义。
- 可以测试 `AgentMessage` 的 JSON 序列化/反序列化
- 可以测试 `EntryId` 的 Display/FromStr roundtrip
- 可以测试 `AgentError` 的 Display 格式
- 可以测试 `HarnessError` 的 `From` 实现（`?` 运算符支持）

**风险：** 无。可以在不了解其他 crate 的情况下完成。

---

### 阶段 2：`llm-harness-loop`（依赖 types）

**依赖状态：** ✅ types 已完成 + llm-api-adapter 可用

**前置条件：** 需要明确 `llm-api-adapter` 的 `LlmClient` trait 签名。当前设计文档没有定义此 trait。loop 实现者需要知道：
- `LlmClient` 的调用方法签名（参数和返回值）
- 返回值是否为 `Stream<Item = llm_api_adapter::Message>`
- 如何传递 `StreamOptions` 给 `LlmClient`

> 🔴 **阻塞风险：** loop crate 的核心函数 `agent_loop()` 无法实现，直到 `LlmClient` trait 的完整签名被明确。设计文档 §3 说 `LlmClient` "由 llm-api-adapter 提供"，但未展示其签名。建议在 §4.2 的 `agent_loop` 函数签名旁，以注释形式写出 `LlmClient` 的核心方法：

```rust
// 假设 LlmClient 签名如下（来自 llm-api-adapter）:
// pub trait LlmClient: Send + Sync {
//     fn stream(
//         &self,
//         model: &str,
//         messages: &[Message],
//         system: Option<&str>,
//         tools: &[ToolDef],
//         options: &StreamOptions,
//     ) -> impl Stream<Item = Result<StreamEvent, Error>> + Send;
// }
```

**可交付物：**
- `ConvertToLlmHook` trait + `DefaultConvertToLlm`（纯数据转换，不依赖外部 IO）
- `HookedTool` wrapper（纯委托模式）
- `LoopConfig` struct
- Tool batch 分治调度逻辑
- `agent_loop()` / `agent_loop_continue()` 完整实现

**可测试性：** ✅ 需要以下 mock/stub（均可在 loop crate 的 `#[cfg(test)]` 模块中实现）：

| Mock 对象 | 用途 | 复杂度 |
|---|---|---|
| `MockLlmClient` | 模拟 LLM 返回流（可控的 stop_reason、tool_calls、text） | 中 |
| `MockTool` | 模拟工具执行（返回预设结果/错误） | 低 |
| `StubExecutionEnv` | 工具执行的 mock 环境 | 低 |
| `StubConvertToLlm` | 测试用的简单消息转换器 | 低 |

**建议随 loop crate 提供 `MockLlmClient` 作为 `#[cfg(test)]` 或 feature-gated 的 `test-utils` 模块**。这同时降低 loop 自身测试和 harness 测试的门槛。

关键测试场景：
- LLM 返回 tool_use → loop 执行 tool → 返回结果给 LLM → LLM 返回文本 → loop 结束
- Sequential tool 插入后 batch 分治正确
- LLM error → Error 事件发出 → AgentEnd 携带错误
- abort 信号中断正在执行的 tool
- steer/follow_up channel 消息正确注入
- terminate 标志全票通过时提前停止
- should_stop hook 阻止/允许停止

---

### 阶段 3：`llm-harness` 中的 Agent（可独立于 Session 实现）

**依赖状态：** ✅ types + loop

**可交付物：**
- `Agent` struct + `AgentState` + 全部方法
- 事件处理循环：prompt → agent_loop() → Stream → 更新 state → broadcast 事件
- 队列管理（steer/follow_up channel）
- `continue_run()` 逻辑
- `reset()` 逻辑

**可测试性：** ✅ 使用 MockLlmClient（从 loop crate 的 test-utils 获取，或自己 mock）。
- 测试 `prompt()` 的状态转换：Idle → Running → Idle
- 测试 `steer()` 在 running 期间的队列入队
- 测试 `abort()` 中断
- 测试并发 prompt 调用的 `NotIdle` 错误
- 测试 `subscribe()` 收到完整事件序列
- 测试 `continue_run()` 从现有 messages 继续

**Agent 不依赖 Session**——它维护自己的 `messages: Vec<AgentMessage>`。这验证了设计决策的正确性：Agent 作为独立组件是可测试的。

---

### 阶段 4：`llm-harness` 中的 Session（可独立于 AgentHarness 实现）

**依赖状态：** ✅ types（Session 自身不依赖 loop）

**可交付物：**
- `SessionEntry`, `SessionEntryPayload`, `CompactionEntry`, `BranchSummaryEntry`
- `SessionStorage` trait（包含 11 个方法）
- `SessionRepo` trait（包含 5 个方法）
- `InMemorySessionStorage` + `InMemorySessionRepo`（测试用 + 生产可用）
- `JsonlSessionStorage` + `JsonlSessionRepo`（生产用）
- `Session` 高层接口（11 个方法）
- `BuiltContext`, `BranchInfo`
- 内存树缓存逻辑

**可测试性：** ✅ Session 不依赖 LlmClient——它是纯数据存储。
- `InMemorySessionStorage` 可用于所有测试，无需文件系统
- 测试树操作：append → children → path_to_root → all_leaves
- 测试 fork：在有子节点的 entry 上 fork，验证新分支
- 测试 navigate_to + build_context：切换分支后上下文正确
- 测试 compaction entry 的 first_kept_entry 追溯
- 测试并发：两个 task 同时 append，验证 Mutex 串行化
- `JsonlSessionStorage` 需要临时目录的集成测试（`#[cfg(test)]` + `tempfile`）

---

### 阶段 5：`llm-harness` 中的 Compaction（依赖 Session + LlmClient）

**依赖状态：** ✅ types + Session 数据结构（SessionStorage trait）+ LlmClient

**可交付物：**
- `CompactionSettings`, `CompactionPreparation`, `CompactionResult`, `FileOperation`
- `prepare_compaction()` — 纯函数，基于 session entries 做决策
- `compact()` — 调用 LLM 生成摘要
- Token 估算逻辑
- Cut point 查找逻辑
- Split-turn 处理
- 文件操作追踪

**可测试性：** ✅ 分两层测试：
- **纯逻辑层**（不依赖 LlmClient）：`prepare_compaction()` 接受 mock 的 `Vec<SessionEntry>`，验证 cut point、token 估算、split-turn 检测
- **集成层**：`compact()` 需要 MockLlmClient（模拟摘要模型返回），验证摘要生成和 CompactionResult 结构

`prepare_compaction` 的纯函数设计使得其单元测试极为简单——构造一组 `SessionEntry`，调用函数，验证输出。

---

### 阶段 6：Skills + PromptTemplates（可独立实现）

**依赖状态：** ✅ types（ExecutionEnv trait）+ serde_yaml

**可交付物：**
- `load_skills()` — 递归扫描、YAML frontmatter 解析、名称/描述校验
- `load_sourced_skills()`
- `format_skills_for_system_prompt()` — XML 格式化
- `format_skill_invocation()` — 显式调用包装
- `load_prompt_templates()`
- `invoke_template()` — 位置参数 + shell 引号解析

**可测试性：** ✅ 需要 ExecutionEnv 实现。

> ⚠️ **缺失测试基础设施：** 设计文档提供了 `InMemorySessionStorage` 和 `InMemorySessionRepo`，但没有 `InMemoryEnv`。Skills 和 Templates 的加载测试需要一个模拟文件系统的 `ExecutionEnv` 实现。建议在 harness crate（或独立的 test-utils）中提供：

```rust
/// 用于测试的内存文件系统——实现 ExecutionEnv trait
pub struct InMemoryEnv {
    files: HashMap<PathBuf, Vec<u8>>,
    dirs:  HashSet<PathBuf>,
    cwd:   PathBuf,
}
```

或者使用 `tempfile` crate 创建临时目录，用真实的 `OsEnv` 实现来测试——这是更简单的方案，不需要维护 mock 实现。

---

### 阶段 7：AgentHarness（依赖以上全部）

**依赖状态：** ✅ types + loop + Agent + Session + Compaction + Skills/Templates

**可交付物：**
- `AgentHarness` struct + `HarnessState` + 全部方法
- `HarnessHooks` → `LoopConfig` 翻译逻辑
- `AgentHarnessEvent` 事件流
- 事件处理管道（伪代码已提供，L1292-1362）
- Pending session writes 缓冲 + save point flush
- `next_turn` 缓冲注入
- 分支操作（fork_branch, navigate_tree, delete_branch, generate_branch_summary）
- `prepare_next_turn` wrapper（读取 HarnessState）

**可测试性：** ✅ 需要集成测试环境：
- `InMemorySessionStorage` + `InMemorySessionRepo`（已在阶段 4 实现）
- `MockLlmClient`（已在阶段 2 实现）
- Skills 和 Templates 的测试数据（SKILL.md 文件内容可内联在测试中）
- Mock hooks 用于验证 hook 调用时序

关键集成测试场景：
- `prompt("hello")` → LLM 返回 "Hi" → session 写入 message → AgentEnd
- `prompt("run tool")` → LLM 返回 tool_use → tool 执行 → session 写入 → AgentEnd
- `steer("stop")` 在 tool 执行后注入 → LLM 处理 steer → 停止
- `compact()` → session 读取 → prepare_compaction → compact → 写入 Compaction entry
- `fork_branch(entry)` → session fork → 写入 BranchPoint → active_cursor 更新
- `navigate_tree(target)` → session 切换 → build_context 反映新分支
- `set_model()` 在 turn 之间 → session 写入 ModelChange → 下一 turn 使用新 model
- `abort()` 中断 running → 队列清空 → 事件发出 → phase → Idle

---

## 4. 各阶段的并行化机会

```
阶段 1 (types) ─────────────────────────────────────────────────
                │
                ▼
阶段 2 (loop) ──────────────────────────────────────────────────
                │
                ▼
        ┌───────┴──────────┬──────────────────┐
        ▼                  ▼                  ▼
阶段 3 (Agent)    阶段 4 (Session)    阶段 6 (Skills/Templates)
        │                  │                  │
        │                  ▼                  │
        │          阶段 5 (Compaction)        │
        │                  │                  │
        └──────────────────┴──────────────────┘
                           │
                           ▼
                阶段 7 (AgentHarness)
```

阶段 3/4/6 **可以并行开发**——它们共享 types 和 loop 依赖，但彼此之间无依赖：
- Agent 只依赖 types + loop（不依赖 Session/Skills）
- Session 只依赖 types（不依赖 loop/Agent）
- Skills/Templates 只依赖 types + ExecutionEnv（不依赖 Session/Agent）

阶段 5 (Compaction) 依赖阶段 4 的 SessionEntry 数据结构，但不需要 Session 的高层 API 完成——可以在 SessionStorage trait 定义完毕后就开始。

---

## 5. 边界问题清单

| # | 严重度 | 位置 | 描述 | 修复建议 |
|---|---|---|---|---|
| 1 | 🔴 | L127-136 vs L1159-1160 | `HarnessError::NotIdle(crate::HarnessPhase)` — HarnessPhase 定义在 harness crate，但 HarnessError 在 types crate | 将 HarnessPhase 提升到 types crate §3.7 |
| 2 | 🔴 | §4.2 vs llm-api-adapter | `agent_loop()` 依赖 `LlmClient` trait，但该 trait 的签名在设计文档中未展示——loop 实现者无法开始编码 | 在 §4.2 以注释形式写出 LlmClient 的核心方法签名（或引用 llm-api-adapter 的文档链接） |
| 3 | 🟡 | L283-291 | `ToolContext.update_tx: tokio::sync::mpsc::Sender<ToolResult>` — types 依赖列表未声明 `tokio` (仅声明 `tokio-util`) | 将 `tokio` (feature = "sync") 加入 types 的 Cargo.toml 依赖 |
| 4 | 🟡 | L690, L1008 | `ModelInfo` 在 harness 中使用，通过 loop → llm-api-adapter 传递——需要 loop crate 显式 `pub use` | 在 §4 开头添加 loop crate 的重导出声明 |
| 5 | 🟡 | §5.5 Skills 测试 | Skills/Templates 加载测试需要 `ExecutionEnv` 实现——设计中有 InMemorySessionStorage 但没有 InMemoryEnv | 添加 `InMemoryEnv` 结构到 harness crate 的 `#[cfg(test)]` 模块，或使用 tempfile + OsEnv |
| 6 | 🟢 | §4 测试基础设施 | Loop crate 的测试需要 MockLlmClient——设计未提及 | 在 loop crate 中提供 feature-gated `test-utils` 模块导出 MockLlmClient |
| 7 | 🟢 | §5.6 集成测试 | AgentHarness 集成测试需要组装多个 mock——复杂度高但可行 | 提供一个 `HarnessTestHarness` builder 简化测试环境搭建 |
| 8 | 🟢 | §5.1 vs §5.6 | Agent 和 AgentHarness 有两套独立的状态管理（AgentState vs HarnessState），事件处理逻辑也有重复——两者都处理 AgentEvent stream | 可接受的重复：Agent 和 AgentHarness 面向不同使用场景，统一反而增加耦合 |

---

## 6. 汇总

### 模块边界清晰度评分

| Crate | 接口大小 | 内聚度 | 依赖纯度 | 可测试性 |
|---|---|---|---|---|
| llm-harness-types | ~45 项 | ⭐⭐⭐⭐ | ⭐⭐⭐ (需加 tokio dep) | ⭐⭐⭐⭐⭐ (零 mock) |
| llm-harness-loop | ~7 项 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ (LlmClient 未清晰) | ⭐⭐⭐⭐ (需 MockLlmClient) |
| llm-harness | ~50 项 | ⭐⭐⭐ (5 子系统混装) | ⭐⭐⭐ (ModelInfo 传递链) | ⭐⭐⭐ (需多个 mock 组装) |

### 分阶段可实现性

**结论：可以分阶段实现。** 依赖拓扑是 `types → loop → harness`，每个阶段有清晰的可交付物和测试策略。

**阻塞项（必须修复才能开始实现）：**
1. `HarnessError::NotIdle` 的 crate 边界违规 — 将 `HarnessPhase` 移至 types
2. `LlmClient` trait 签名不明确 — 补充接口文档或引用

**非阻塞但建议在实现前澄清：**
3. types 缺少 `tokio` 依赖声明
4. loop crate 的 `pub use` 重导出声明
5-8. 测试基础设施（MockLlmClient, InMemoryEnv）

**适合并行开发的模块组：**
- Agent + Session + Skills/Templates（阶段 3/4/6 三个子系统互不依赖，可三人并行）
