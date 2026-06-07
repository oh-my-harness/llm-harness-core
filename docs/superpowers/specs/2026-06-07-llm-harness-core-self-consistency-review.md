# llm-harness-core 设计自洽性审核

**日期：** 2026-06-07
**审核对象：** [2026-06-07-llm-harness-core-design.md](./2026-06-07-llm-harness-core-design.md) (v4)
**审核维度：** 设计内部自洽性——类型一致性、数据流完整性、架构矛盾、未定义引用

---

## 🔴 架构矛盾

### 1. `ConvertToLlmHook` 违反 crate 依赖边界

**涉及行：** L309-314 (trait 定义) vs L46 (types 依赖列表) vs L30-39 (依赖图)

```rust
// 定义在 llm-harness-types 中
pub trait ConvertToLlmHook: Send + Sync {
    fn convert<'a>(&'a self, messages: &'a [AgentMessage])
        -> BoxFuture<'a, Result<Vec<llm_api_adapter::Message>, AgentError>>;
}
```

`llm-harness-types` 的依赖列表（L46）是 `serde, serde_json, futures, tokio-util, thiserror, uuid, chrono`——**不含 `llm-api-adapter`**。但 `ConvertToLlmHook::convert` 的返回值直接引用了 `llm_api_adapter::Message`。

依赖图（L30-39）显示 `llm-api-adapter` → `llm-harness-types`（前者的箭头指向后者），但箭头方向语义不明确。如果箭头代表"被依赖"，即 `llm-harness-types` 依赖 `llm-api-adapter`，则与 L46 的声明矛盾。如果箭头代表"依赖"，即 `llm-api-adapter` 依赖 `llm-harness-types`，则 `llm-harness-types` 不应知道 `llm_api_adapter::Message` 的类型。

**两种修复方向：**
- **方案 A：** 将 `ConvertToLlmHook` 移到 `llm-harness-loop`（它已经依赖 `llm-api-adapter`）。但这会使得 `HarnessHooks`（在 `llm-harness` 中）也需要引用此 trait，需要 `llm-harness` 直接依赖 `llm-api-adapter`。
- **方案 B：** 在 `llm-harness-types` 中定义 `Message` 的抽象（generic associated type 或简单的 enum），由 loop 层做适配。这是零 IO 纯类型层的正确做法。

---

### 2. LoopConfig 与 HarnessHooks 的 hook 大量重复

**涉及行：** L424-451 (LoopConfig) vs L944-956 (HarnessHooks)

以下 6 个 hook **同时存在于两个 struct 中**：

| Hook | LoopConfig | HarnessHooks |
|---|---|---|
| `convert_to_llm` | L430 (必需) | L945 (必需) |
| `transform_context` | L433 (可选) | L950 (可选) |
| `prepare_next_turn` | L436 (可选) | L951 (可选) |
| `should_stop` | L439 (可选) | L952 (可选) |
| `before_provider_request` | L442 (可选) | L953 (可选) |
| `after_provider_response` | L443 (可选) | L954 (可选) |

另外 `auth` 在 LoopConfig (L446) 和 AgentHarness (L925) 中也重复。

文档没有描述 HarnessHooks → LoopConfig 的**翻译机制**。如果 Harness 调用 `agent_loop()` 时需要从 `HarnessHooks` 构造 `LoopConfig`，那么：
- 翻译逻辑在哪里？是构造函数、builder 还是每次 `prompt()` 调用时动态构造？
- 如果两者独立设置（如 LoopConfig 有 `convert_to_llm: A`，HarnessHooks 也有 `convert_to_llm: B`），哪个生效？
- `before_tool_call` / `after_tool_call` 仅存在于 HarnessHooks，通过 `HookedTool` 包装（L468-469）——但 `HookedTool` 类型在文档中**从未定义**。

**建议：** 消除重复——`HarnessHooks` 是唯一真相源，`LoopConfig` 由 Harness 在调用 `agent_loop()` 时从 `HarnessHooks` 构造。LoopConfig 不应独立暴露 hook 字段，或明确标注 "由 Harness 填充，调用方不应直接设置"。

---

### 3. AgentHarness 的 `next_turn` 缺乏注入通道

**涉及行：** L1031 (next_turn 方法) vs L449-450 (LoopConfig 仅有 steer_rx/follow_up_rx)

```rust
// AgentHarness 提供的方法
pub fn next_turn(&self, text: impl Into<String>);
```

但 `LoopConfig` 只有两个 channel：
```rust
pub steer_rx:     Option<mpsc::Receiver<String>>,
pub follow_up_rx: Option<mpsc::Receiver<String>>,
```

**没有 `next_turn_rx` channel。** 在 TS 中，`nextTurn` 消息在下一次 `prompt()` 调用时被注入到初始消息之前。Rust 设计中，AgentHarness 直接调用 `agent_loop()`，所以 `next_turn` 消息应该由 Harness 在调用 `agent_loop()` 之前手动合并到初始 prompt 消息中。这是可行的——但设计文档没有描述这个流程。

**建议：** 在 AgentHarness 的方法描述中补充 `next_turn` 消息的注入时机和方式。或者在 LoopConfig 中加入 `next_turn_rx` channel 以保持一致性。

---

## 🟡 类型一致性问题

### 4. 多个错误类型声明但未定义

以下类型在方法签名和 trait 中被引用，但全文没有任何地方给出定义：

| 类型 | 首次引用位置 | 使用范围 |
|---|---|---|
| `EnvError` | L257 `ExecutionEnv::read_text_file` 返回值 | ExecutionEnv 全部 ~13 个方法 |
| `SessionError` | L669 `SessionStorage` 全部方法返回值 | SessionStorage + SessionRepo + Session |
| `HarnessError` | L994 `AgentHarness` API 返回值 | AgentHarness 全部 ~20 个方法 |
| `CompactionError` | L831 `compact()` 返回值 | Compaction 模块 |
| `TemplateError` | L907 `invoke_template()` 返回值 | PromptTemplate 模块 |
| `CreateSessionOptions` | L711 `SessionRepo::create()` 参数 | SessionRepo |
| `ListSessionOptions` | L717 `SessionRepo::list()` 参数 | SessionRepo |
| `SourcedSkill` | L878 `load_sourced_skills()` 返回值 | Skills 模块 |
| `DiagnosticLevel` | L865 `SkillDiagnostic.level` 字段 | Skills 模块 |
| `ProviderResponseInfo` | L363 `AfterProviderResponseHook::after_response()` 参数 | Hook trait |

这些不是 "可以推断" 的类型——它们各自承载不同的错误语义（如 `EnvError` 有 `FileErrorCode` 枚举对应 TS 的 8 种错误码）。没有定义意味着接口不可编译。

---

### 5. Hook 上下文类型的字段仅为注释

**涉及行：** L339, L345, L350

```rust
pub struct BeforeToolCallCtx<'a> { /* ... assistant_message, tool_use_id, tool_name, args ... */ }
pub struct AfterToolCallCtx<'a>  { /* ... assistant_message, tool_use_id, tool_name, args, result ... */ }
pub struct ShouldStopCtx<'a>     { /* last_assistant, stop_reason, turn_index ... */ }
```

字段**仅在注释中列举**，没有 Rust 类型声明。无法判断：
- 字段生命周期标注是否正确（`&'a AssistantMessage` vs `&'a str`）
- `args` 在 `BeforeToolCallCtx` 中是 `&serde_json::Value` 还是 `serde_json::Value`？
- `AfterToolCallCtx::result` 是 `&ToolResult` 还是 `&Result<ToolResult, ToolError>`？

同样，`PrepareNextTurnCtx`（L324-327）定义了字段但 `BeforeToolCallCtx` 没有——不一致的细化程度。

---

### 6. `active_leaf` 命名与实际语义矛盾

**涉及行：** L688-692 (SessionStorage) vs L763-764 (fork_branch)

```rust
fn active_leaf<'a>(&'a self) -> BoxFuture<'a, Result<Option<EntryId>>>;
fn set_active_leaf<'a>(&'a self, id: EntryId) -> Result<(), SessionError>;
```

`fork_branch` (L763-764) 将 `active_leaf` 设置为 `from_entry`（历史中的一个内部节点，**不是 leaf**——它已经有子节点）：

> "写入 BranchPoint 标记并切换 active_leaf 到新分支起点"

"新分支起点" `from_entry` 已经有子节点（旧分支），所以它不是 leaf。将 `active_leaf` 指向非 leaf 节点，与名称矛盾。这个字段实际上应该叫 `active_cursor` 或 `write_target`——它表示 "下一个 append 的 parent_id 应该指向哪个 entry"。

**建议：** 要么将字段重命名为 `active_cursor` / `write_target`；要么在 fork 时不改变 `active_leaf`，而是引入单独的 `write_target` 概念。

---

### 7. `SessionEntry` 与 `SessionEntryPayload` 的 ID/时间戳分配不明确

**涉及行：** L611-616 (SessionEntry) vs L618-637 (SessionEntryPayload) vs L752 (Session::append)

```rust
pub struct SessionEntry {
    pub id:         EntryId,
    pub parent_id:  Option<EntryId>,
    pub timestamp:  chrono::DateTime<chrono::Utc>,
    pub payload:    SessionEntryPayload,
}
```

`Session::append` (L752) 接受 `SessionEntryPayload`（不含 id/parent_id/timestamp）：

```rust
pub async fn append(&self, payload: SessionEntryPayload) -> Result<EntryId, SessionError>;
```

问题：谁负责填充 `id`、`parent_id`、`timestamp`？
- `id` — 由 `SessionStorage::create_entry_id()` (L671) 生成
- `parent_id` — 应等于当前的 `active_leaf`
- `timestamp` — 当前时间

这些应该由 `Session::append` 内部填充，但文档没有说明。调用方直接使用 `SessionStorage::append_entry` 时，需要自己填充完整的 `SessionEntry`——这时调用方需要知道如何生成 id 和 parent_id，容易出错。

**建议：** 明确 `Session::append` 的内部行为：调用 `storage.create_entry_id()` 生成 id，读取 `storage.active_leaf()` 作为 parent_id，使用当前时间作为 timestamp。

---

### 8. `AgentContext` 缺少 `tools` 字段

**涉及行：** L391-394 (AgentContext) vs L425 (LoopConfig.tools)

```rust
pub struct AgentContext {
    pub system_prompt: Option<String>,
    pub messages:      Vec<AgentMessage>,
}
```

`AgentContext` 没有 `tools` 字段。工具信息通过 `LoopConfig.tools` 提供。这意味着：
- `TransformContextHook::transform`（L317-319）接收 `AgentContext` 但无法获知当前有哪些工具可用
- `AgentContext` 只包含 "发送给 LLM 的内容"，不包含 "可用的能力"

这是有意为之的设计决策（关注点分离），但与 TS `AgentContext`（含 `tools?: AgentTool<any>[]`）不同。如果是有意偏离，应在设计决策表中记录；如果无意遗漏，需要补充。

---

### 9. `TurnSnapshot` 定义位置与引用不一致

**涉及行：** L397-402 (TurnSnapshot 在 types crate) vs L551-553 (Agent 使用 TurnSnapshot)

`TurnSnapshot` 定义在 §3.7（`llm-harness-types`），但构造方式在 §5.1 Agent 的锁使用约束中描述（L551-553）：

```rust
let snapshot = {
    let st = self.state.lock().unwrap();
    TurnSnapshot { /* clone fields */ }
};
```

`TurnSnapshot` 含 `tools: Vec<Arc<dyn Tool>>`（L400）。`Arc<dyn Tool>` 的 clone 是廉价（引用计数+1）。但 `TurnSnapshot` 本身 derive `Clone`（L396），这意味着每个 turn 的快照可以廉价复制——这是正确的。

但问题是：`TurnSnapshot` 在 types crate 中定义，而构造它需要读取 AgentState 或 HarnessState（分别在 `llm-harness` 中）。`TurnSnapshot` 是否需要 `From<&AgentState>` 或 `From<&HarnessState>` 的转换实现？如果是，这个 impl 放在哪个 crate？

当前设计没有说明 TurnSnapshot 的来源。它在 types 中是独立 struct，但在 Agent 中被直接构造（需要访问 AgentState 的内部字段），这意味着 Agent/Harness 需要知道 TurnSnapshot 的字段结构——这在同一个 workspace 内是可行的，但缺少显式的构造方法。

---

## 🟡 数据流与控制流问题

### 10. AgentHarness 的事件处理管道未定义

**涉及行：** L1047 ("Harness 不包装 Agent；它直接调用 agent_loop()")

AgentHarness 直接调用 `agent_loop()` 获取 `Stream<Item = AgentEvent>`。但文档没有描述 Harness **如何处理这些事件**：

- `MessageEnd` 事件如何转化为 `pending_session_writes.push(SessionEntryPayload::Message(...))`？
- `TurnEnd` 事件到达时，是否 flush pending writes？（对应 TS 的 save_point）
- `AgentEnd` 事件如何触发 `phase = Idle` + `wait_for_idle` 唤醒？
- 工具执行事件如何更新 `pending_tool_calls`？

TS 的 `AgentHarness.handleAgentEvent` 有完整的 switch-case（约 30 行），Rust 设计完全没有描述这个核心事件处理循环。

**建议：** 这不应是 "实现细节"——事件处理是 AgentHarness 正确性的核心。至少应提供伪代码或状态转换图。

---

### 11. Steer/Follow-up 消息的 String → AgentMessage 转换缺失

**涉及行：** L449-450 (steer_rx/follow_up_rx 为 `Receiver<String>`) vs L118-121 (UserMessage 需要 `timestamp`)

Loop 从 channel 收到 `String`，需要将其包装为 `AgentMessage::User(UserMessage { content, timestamp })`。但：
- `timestamp` 从哪里来？Loop 内部生成（需要依赖 `chrono`）？还是 channel 应该传 `AgentMessage` 而非 `String`？
- TS 的 steer/followUp 接口接受 `AgentMessage`，支持图片等多模态内容。Rust 硬编码为 `String`，不支持图片注入。

这是一个**有意简化**还是**设计疏忽**？应明确标注。

---

### 12. `prepare_next_turn` 不能变更 tools

**涉及行：** L330-332 (NextTurnDirective) vs L938 (active_tools) vs L1006 (set_active_tools)

```rust
pub struct NextTurnDirective {
    pub context:        Option<AgentContext>,
    pub model:          Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
}
```

`NextTurnDirective` 可以改 context（含 system_prompt + messages）、model、thinking_level，**但不能改 tools**。

但 AgentHarness 提供了 `set_active_tools()` (L1006)，可以在运行时切换活跃工具集。如果用户在 turn 之间调用 `set_active_tools()`，变更如何传播到下一个 turn？

在 TS 中，`prepareNextTurn` 每次 turn 结束后重建整个上下文（包括从 session 重建 messages、从 Harness 状态取最新 tools/activeTools）。Rust 设计应该有相同的机制——`prepare_next_turn` hook（由 Harness 实现）在重建上下文时会读取最新的 `HarnessState.active_tools`，从而更新 tools。

但这意味着 hook 实现需要访问 HarnessState——而 `PrepareNextTurnHook` 是一个 trait，它的实现（由 Harness 提供）如何捕获对 `HarnessState` 的引用？这引入了从 hook 实现到 Harness 内部状态的**反向引用**，容易产生生命周期问题或死锁。

**建议：** 在 Harness 实现要点中说明 `prepare_next_turn` 闭包/hook 如何安全地读取 Harness 状态。

---

### 13. Compaction 的 `summary_model` 认证来源模糊

**涉及行：** L804 (summary_model: String) vs L829 (auth: Option<&dyn AuthHook>)

`compact()` (L825-830) 接受 `auth: Option<&dyn AuthHook>`。但：
- 如果 `auth` 是 `None`，compact 如何获取 API key？
- `LlmClient` trait（来自 llm-api-adapter）是否已经内置了认证？如果是，为什么还需要 `auth` 参数？
- `summary_model` 只是一个 String（模型名称），compact 如何知道该模型的 `ModelInfo`（如 context_window, max_tokens）？

在 TS 中，compact 接收完整的 `Model<any>` 对象 + `apiKey: string` + `headers`。Rust 设计用 `summary_model: String` + `auth: Option<&dyn AuthHook>` 替代——信息量不足。

---

### 14. Session 分支操作的并发安全性未讨论

**涉及行：** L758-770 (navigate_to, fork_branch, delete_branch)

Session 的方法是 `&self`（不可变引用），这意味着多个调用方可以并发调用。但如果两个调用方同时：
- `navigate_to(A)` 和 `navigate_to(B)` → 哪个生效？
- `delete_branch(X)` 和 `append_message(msg)` → 消息追加到已删除的分支？

`SessionStorage` 的 `set_active_leaf` 和 `append_entry` 之间没有事务边界。JSONL 存储特别脆弱——文件追加和 `.meta.json` 更新不是原子的。

**建议：** 要么在 Session 层使用 `&mut self` 强制串行化，要么在文档中标注并发约束。

---

### 15. JSONL 存储的树查询性能隐患

**涉及行：** L785-786

> "单 session 单文件，所有 entry 按时间顺序 append。文件本身是线性日志，树结构由 parent_id 在读取时重建。"

对于以下操作，每次都需要**全量读取并构建树**：
- `path_to_root(target)` — O(n) 扫描所有 entry
- `children(parent)` — O(n) 扫描
- `all_leaves()` — O(n) 扫描 + 树构建
- `common_ancestor(a, b)` — 两条 path_to_root → O(n)

当 session 有数万条 entry 时（长对话常见），每次 `build_context()` 都需要 `path_to_root(active_leaf)` → 全量扫描。加上 compaction 的解释逻辑，每次构建上下文的 I/O 和时间成本不可忽略。

在内存中缓存树结构可以解决，但 `SessionStorage` trait 是按需查询的接口——缓存属于实现细节，trait 层面没有提供缓存失效机制（如 "entry N appended, invalidate cache"）。

**建议：** 要么在 trait 文档中明确 "调用方应预期每次查询均为全量扫描，大数据量下自行缓存"，要么在 Session 层引入缓存。

---

## 🟢 次要问题

### 16. `CompactionSettings` 存在语义重叠的字段

**涉及行：** L799-806

```rust
pub struct CompactionSettings {
    pub token_threshold:    usize,  // 超过触发压缩
    pub keep_recent_tokens: usize,  // 保留尾部 N tokens 不压缩
    pub reserve_tokens:     usize,  // 为 LLM 响应预留
    ...
}
```

`token_threshold` 和 `reserve_tokens` 的语义接近——前者是绝对值阈值，后者是为响应预留的空间。TS 中只有 `reserveTokens` + `keepRecentTokens`，阈值为 `contextWindow - reserveTokens`。Rust 有三个参数，它们的优先级和组合逻辑没有说明。例如：`token_threshold=100000, reserve_tokens=16384`——实际触发条件是 token 数超过 100000 还是超过 `contextWindow - 16384`？

---

### 17. `SessionRepo::fork` 的跨 session fork 语义不完整

**涉及行：** L723-727

```rust
fn fork<'a>(&'a self, source_id: &'a str, from_entry: EntryId, opts: ForkOptions)
    -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;
```

`ForkOptions` (L730-734) 有 `copy_entries: bool`——false 时 "仅复制 path 元数据并打 BranchPoint 引用 source"。但：
- "打 BranchPoint 引用 source" 中的 `BranchPoint` 引用跨 session 的 entry——`EntryId` 在不同 session 间是否具有全局唯一性？UUIDv7 理论上是，但设计未声明。
- `copy_entries: false` 时，新 session 如何 "看到" source session 的历史？通过 `parentSessionPath` 引用（类似 TS 的 `JsonlSessionMetadata.parentSessionPath`）？但设计未定义此字段。

---

### 18. Default `ConvertToLlmHook` 实现未提供

**涉及行：** L307-308

> "默认实现处理 User/Assistant/ToolResult/BranchSummary/CompactionSummary；CustomMessage 必须由调用方覆盖处理。"

这是 trait 的 **默认方法实现**，还是一个**独立的 `DefaultConvertToLlm` 结构体**？文档没说。如果是 trait 默认方法，那么 `ConvertToLlmHook::convert` trait 方法就不能是 required——但 L310-313 显示它是 required（没有 `{}` body）。

---

### 19. `Session::build_context()` 返回纯 messages 而非包含 system_prompt

**涉及行：** L748

```rust
pub async fn build_context(&self) -> Result<Vec<AgentMessage>, SessionError>;
```

返回 `Vec<AgentMessage>` 不含 system_prompt。但 TS 的 `buildSessionContext` 同时返回 messages 和 last known model/thinkingLevel。Rust 的 `build_context` 只返回 messages——model/thinkingLevel 从哪里恢复？从 `SessionStorage::path_to_root` 走查 entry 时需要提取 `ModelChange` 和 `ThinkingLevelChange`，但这个逻辑不在 `build_context` 中，而在 AgentHarness 的 `createTurnState` 中。需要确保 `Session` 或 `SessionStorage` 提供了逐 entry 遍历路径的能力（`read_active_path` 可以做到，但调用方需要自己解析 entry 类型）。

---

### 20. `ToolExecutionUpdate` 与 `ToolCallArgsDelta` 的名称混淆

**涉及行：** L186-187 vs L191

```rust
ToolCallArgsDelta { tool_use_id: String, partial_input: String },  // LLM 流式返回 tool 参数
ToolExecutionUpdate { tool_use_id: String, partial: ToolResult },   // Tool 执行中间结果
```

有两个不同概念的 "增量"：LLM 流式返回 tool 参数（`ToolCallArgsDelta`）和 tool 执行中间结果（`ToolExecutionUpdate`）。命名清晰度可以接受，但区分 `ToolCall*`（LLM 侧）和 `ToolExecution*`（执行侧）的注释（L189）应更显眼。

---

## 汇总

| # | 类别 | 严重度 | 描述 |
|---|---|---|---|
| 1 | 架构矛盾 | 🔴 | `ConvertToLlmHook` 引用外部类型违反 types crate 零 IO 约束 |
| 2 | 架构矛盾 | 🔴 | LoopConfig 与 HarnessHooks 6 个 hook 重复 + HookedTool 未定义 |
| 3 | 架构矛盾 | 🔴 | `next_turn` 缺乏 LoopConfig 注入通道 |
| 4 | 类型缺失 | 🔴 | 10 个错误/配置类型 (EnvError, SessionError, HarnessError, ...) 声明但未定义 |
| 5 | 类型不完整 | 🟡 | BeforeToolCallCtx/AfterToolCallCtx/ShouldStopCtx 字段仅为注释 |
| 6 | 命名矛盾 | 🟡 | `active_leaf` fork 后指向非 leaf 节点 |
| 7 | 数据流缺失 | 🟡 | SessionEntry 的 id/parent_id/timestamp 分配责任不明确 |
| 8 | 设计偏离 | 🟡 | AgentContext 缺少 tools 字段，需确认是否 intentional |
| 9 | 构造路径 | 🟡 | TurnSnapshot 跨 crate 构造方式未说明 |
| 10 | 数据流缺失 | 🟡 | AgentHarness 的事件处理管道完全空白 |
| 11 | 类型窄化 | 🟡 | steer/follow_up 硬编码 String 丢失多模态能力 |
| 12 | 数据流 | 🟡 | prepare_next_turn 不能改 tools，与 set_active_tools 的交互不清 |
| 13 | 外部依赖 | 🟡 | compact() 的认证和 ModelInfo 来源不清晰 |
| 14 | 并发安全 | 🟡 | Session 分支操作的并发安全性未讨论 |
| 15 | 性能 | 🟡 | JSONL 存储每次树查询均需全量扫描 |
| 16 | 语义重叠 | 🟢 | CompactionSettings 三个字段语义重叠 |
| 17 | 语义不完整 | 🟢 | 跨 session fork 的非复制模式未定义引用机制 |
| 18 | 实现缺失 | 🟢 | Default ConvertToLlmHook 实现未提供 |
| 19 | 接口不完整 | 🟢 | build_context() 不返回 model/thinkingLevel |
| 20 | 命名 | 🟢 | ToolCallArgsDelta 与 ToolExecutionUpdate 区分需更显眼 |

**最优先修复项（阻塞实现）：**
- **#1** — `ConvertToLlmHook` 依赖边界：需要决定是移动 trait 位置还是在 types 中引入抽象 Message 类型
- **#2** — Hook 重复：需要定义 HarnessHooks → LoopConfig 的清晰翻译路径
- **#4** — 10 个未定义类型：每个都需要定义，否则接口无法编译
- **#10** — 事件处理管道：这是 AgentHarness 正确性的核心，不应留到实现阶段发现
