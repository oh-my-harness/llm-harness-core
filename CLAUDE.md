# llm-harness-core 开发原则

## 工作风格

### 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

LLMs often pick an interpretation silently and run with it. This principle forces explicit reasoning:

- **State assumptions explicitly** — If uncertain, ask rather than guess
- **Present multiple interpretations** — Don't pick silently when ambiguity exists
- **Push back when warranted** — If a simpler approach exists, say so
- **Stop when confused** — Name what's unclear and ask for clarification

### 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

Combat the tendency toward overengineering:

- No features beyond what was asked
- No abstractions for single-use code
- No "flexibility" or "configurability" that wasn't requested
- No error handling for impossible scenarios
- If 200 lines could be 50, rewrite it

**The test:** Would a senior engineer say this is overcomplicated? If yes, simplify.

### 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:

- Don't "improve" adjacent code, comments, or formatting
- Don't refactor things that aren't broken
- Match existing style, even if you'd do it differently
- If you notice unrelated dead code, mention it — don't delete it

When your changes create orphans:

- Remove imports/variables/functions that YOUR changes made unused
- Don't remove pre-existing dead code unless asked

**The test:** Every changed line should trace directly to the user's request.

### 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform imperative tasks into verifiable goals:

| Instead of... | Transform to... |
|--------------|-----------------|
| "Add validation" | "Write tests for invalid inputs, then make them pass" |
| "Fix the bug" | "Write a test that reproduces it, then make it pass" |
| "Refactor X" | "Ensure tests pass before and after" |

For multi-step tasks, state a brief plan:

```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let the LLM loop independently. Weak criteria ("make it work") require constant clarification.

### 5. 代码语言规范

**仅使用英文和中文。禁止使用日文、韩文等其他语言。**

代码注释、文档字符串、提交信息统一用英文或中文：

- 新增代码优先用中文注释（与现有 CLAUDE.md 和项目文档保持一致）
- 技术术语保持英文（如 `LLM`、`Hook`、`Token`）
- 禁止用日文、韩文、其他非英中语言

**审查点：** 在代码审查和提交前，检查是否有日文/韩文残留。

### 6. 提交前必做清理

**每次 commit 前必须：**

1. 运行 `cargo fmt` — 确保代码格式统一
2. 运行 `cargo clippy --all-targets --all-features` — 修复所有 clippy 警告（除非有充分理由保留）

这两个检查是 CI 的硬要求，在本地提前发现能避免提交被拒。


## 项目简介

`@earendil-works/pi-agent-core`（TypeScript）的 Rust 全量重写。目标是学习其核心设计哲学，用 Rust 惯用法重新表达，不是逐行翻译。

参考实现位于 `../pi-main/packages/agent`。设计决策有疑问时，先读 TS 源码再做判断。

## Crate 结构与依赖规则

```
llm-harness-core/
├── Cargo.toml               (workspace)
└── crates/
    ├── llm-harness-types/   (纯类型 + trait，零 IO)
    ├── llm-harness-loop/    (core loop 引擎)
    └── llm-harness/         (Agent + Harness + Session + Compaction + Skills)
```

依赖方向（A → B 表示 A 依赖 B）：

```
llm-harness → llm-harness-loop → llm-api-adapter (外部)
llm-harness → llm-harness-types
llm-harness-loop → llm-harness-types
```

**硬性规则：**
- `llm-harness-types` 不依赖任何外部 crate（包括 `llm-api-adapter`），保持零 IO。
- `llm-harness` 不直接依赖 `llm-api-adapter`，通过 `llm-harness-loop` 的 `pub use` 获取其类型。
- 违反依赖方向的修改不允许。

## 设计文档

所有设计决策记录在 `docs/superpowers/specs/` 下：

| 文件 | 内容 |
|---|---|
| `...-design.md` | 总纲：决策表、不在范围、路线图 |
| `...-phase1-types.md` | llm-harness-types 全部类型 |
| `...-phase2-loop.md` | LoopConfig / HookedTool / agent_loop |
| `...-phase3-agent.md` | Agent 状态机 |
| `...-phase4-session.md` | Session / Storage / 分支操作 |
| `...-phase5-compaction.md` | Compaction 算法 |
| `...-phase6-skills.md` | Skills / PromptTemplates |
| `...-phase7-agent-harness.md` | AgentHarness / HarnessHooks / 事件管道 |

实现前先读相关阶段的 spec。每个结构体/函数的设计理由在 spec 的 `>` 块引用中。

## 核心开发原则

### 分层职责

- **types crate**：声明类型和 trait，不含逻辑。
- **loop crate**：实现 `agent_loop`，保持近似纯函数——接收 `LoopConfig`，驱动 LLM 流，产出事件流。不写 session，不管理状态。
- **harness crate**：唯一有状态的层。**Harness 是 session 的唯一写入方**——compaction、tool 结果等任何需要落盘的操作都通过 `pending_session_writes` + `flush_pending_writes` 路径。

### Hook 与事件的分工

- **Hook**（`HarnessHooks` 里的 trait）：参与框架流程、可修改行为，构造时注入，有类型安全的返回值。
- **事件**（`AgentHarnessEvent` enum）：纯通知，运行时动态订阅，无返回值。
- 不要把 hook 调用点暴露为事件，也不要把纯通知实现为 hook。

### 并发与锁

- 用 `std::sync::Mutex` + 快照模式（lock → clone → drop lock → 操作快照）。不用 `RwLock`。
- 不在持锁状态下跨 `.await`。
- `HookedTool` 无状态：`assistant_message` 和 `turn_index` 通过 `ToolContext` 传入，不存储在结构体上。

### 错误处理与失败语义

- `compact()` 不写 session，只返回 `CompactionResult`。Harness 在 `Ok` 分支写入。
- 加载函数（`load_skills`、`load_prompt_templates`）返回 `(成功列表, Vec<Diagnostic>)`，不因单个文件失败而整体失败。

### 工具调度

- 按 `ToolExecutionMode::Sequential` 为分割点把工具列表切成子组，组内并发（`join_all`），子组间顺序。
- 不做全局串行，也不做无差别并发。

## 测试原则

**TDD 流程（实现阶段）：**

1. **先写测试**：写会编译但断言失败的测试，定义功能的预期行为（Red）
2. **停下来**：提交测试，等用户审查——审查的是"要做什么"，不是"怎么做"
3. **实现**：用户批准测试后，写最小代码让所有测试通过（Green）
4. **重构**：在测试保护下整理代码（Refactor）

不要把测试和实现混在同一步完成。测试是规格的具体化，先让人确认测试再实现，能在最早阶段发现理解偏差。

**基础设施：**
- `llm-harness-loop` 提供 feature-gated `test-utils` 模块，导出 `MockLlmClient`。
- Skills / PromptTemplates 测试用 `tempfile::TempDir` + `OsEnv` 真实 fs，不维护 `InMemoryEnv`。
- 集成测试用 `InMemorySessionRepo` + `MockLlmClient`。

## 实现后自查

每次实现完成后，在提交前自行检查以下维度。有 ❌ 项自行修复，有 ⚠️ 项列出后交用户判断：

**架构合规**
- ❌ `llm-harness-types` 是否引入了外部 crate 依赖？
- ❌ `llm-harness` 是否直接依赖了 `llm-api-adapter`？
- ❌ 是否有非 Harness 路径写入 session（绕过 `pending_session_writes`）？
- ❌ 工具调度是否退化为全局串行或无差别并发（应为分治子组）？

**代码质量**
- ❌ 所有 `pub` 类型、字段、enum variant 是否有 `///` doc comment？
- ⚠️ 是否在持 `Mutex` 锁时跨了 `.await`？
- ⚠️ 是否使用了 `RwLock`（项目统一用 `Mutex` + 快照模式）？

**测试**
- ❌ 新增的 pub 函数是否有对应测试？
- ⚠️ 集成测试是否依赖真实 LLM（应使用 `MockLlmClient`）？

**文档同步**
- ⚠️ 实现是否与 spec 有出入？若有，更新对应的 `docs/superpowers/specs/` 文件。
- ⚠️ 新增或修改了跨 crate 的接口约定？更新本文件（CLAUDE.md）对应段落。

## Git 规则

- **不自动提交**。修改文件后等用户明确要求再 commit。
- Commit message 格式：`<type>(<scope>): <description>`
  - type：`feat` / `fix` / `docs` / `test` / `refactor` / `chore`
  - scope：crate 名或模块名，如 `types`、`loop`、`harness`、`session`
  - 示例：`feat(loop): implement tool batch dispatch`、`test(session): add branch fork tests`
- description 说明 *为什么* 而非 *做了什么*。

## 收尾

每次开发完成后：提交并推送本仓库变更；如有进度变化同步更新顶层 `STATUS.md` 并推送 `oh-my-harness/oh-my-harness`。
