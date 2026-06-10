# llm-harness-core / runtime SDK 边界设计

**日期：** 2026-06-10
**状态：** 提议稿

## 1. 目的

本文定义 `llm-harness-core`、未来的 `llm-harness-runtime`，以及
coding agent、EDA agent 等领域 agent 仓库之间的 SDK 定位和职责边界。

本文不替代现有的 core 架构设计：
`2026-06-07-llm-harness-core-design.md`。本文只是在该架构基础上，明确
core 应该如何作为 SDK 暴露，以及哪些更高层的运行时职责应该放到
`llm-harness-runtime`。

## 2. Core 定位

`llm-harness-core` 是一个完整的 agent framework SDK。它不是只有薄薄一层的
kernel，也不是一个具体产品 agent。

现有三 crate 架构仍然是权威设计：

```text
llm-harness-types
  共享消息、content block、事件、Tool、ExecutionEnv、hooks、错误等基础协议。

llm-harness-loop
  流式 LLM loop、provider bridge、tool call 解析、tool 调度。

llm-harness
  Agent、AgentHarness、Session、Compaction、Skills/Templates、OsEnv。
```

core SDK 负责 agent 执行机制：

- 消息和内容模型。
- 工具和执行环境抽象。
- 流式 loop 和工具调度。
- Agent 事件和 Harness 事件。
- 有状态的 `Agent`。
- 基于 session 的 `AgentHarness`。
- Session storage/repository trait，以及内置 memory/JSONL 实现。
- 分支、导航和上下文重建。
- Compaction 基础能力和编排 hook。
- Skills 和 prompt templates。
- Harness hooks。

core SDK 不负责产品层行为：

- 具体工具实现，例如 read、bash、edit、write、grep、find、ls。
- 工具注册策略；core 只接收 `Vec<Arc<dyn Tool>>` 和 active tool names。
- Settings、auth storage、model registry、API key discovery。
- 产品级 system prompt。
- CLI、TUI、HTTP、RPC、MCP 或包分发。
- Extension/plugin runtime。
- 领域专属工具或资源。

## 3. 仓库边界

仓库划分应理解为三层：

```text
llm-harness-core
  “agent 怎么运行”
  稳定的 framework 机制和扩展契约。

llm-harness-runtime
  “一个可复用的 agent 产品运行时怎么组装”
  建立在 core 之上的共享应用运行时。

domain agents
  “这个 agent 是谁”
  建立在 runtime 和/或 core 之上的产品或领域包。
```

### 3.1 llm-harness-core

`llm-harness-core` 适合这些使用者：

- 需要直接控制 tools、sessions、hooks、message conversion 的 framework 用户。
- 构建更高层 SDK 的 runtime 作者。
- 需要自定义 `ExecutionEnv`、`SessionRepo` 或 `ConvertToLlmHook` 的高级集成者。
- 可以直接使用 `Agent` 的测试或原型场景。

core 必须在不依赖 `llm-harness-runtime` 的情况下保持可用。

### 3.2 llm-harness-runtime

`llm-harness-runtime` 适合承载多个领域 agent 都会复用、但又太具体而不适合放进 core
的共享应用运行时能力：

- 基础工具：read、bash、edit、write、grep、find、ls。
- Tool registry 和 tool selection policy。
- Tool prompt snippets 和工具使用 guidelines。
- System prompt builder framework。
- Settings loading 和分层配置。
- Auth storage 和运行时认证解析。
- Model registry 和模型选择辅助。
- 对 skills、prompt templates、context files、extension 资源的编排。
- 自动 retry 策略。
- 自动 compaction 策略。
- 将多个 extension 映射到 core `HarnessHooks` 的 extension/plugin runtime。
- 更高层的 builder，例如 `AgentRuntimeBuilder`。

runtime 依赖 core。core 不能依赖 runtime。

runtime 不能重新实现 `AgentHarness`、session 持久化、compaction entry 语义、
skills/templates 解析或 streaming loop。这些都是 core 的职责。

### 3.3 Domain Agents

Domain agents 是产品或领域包，例如 coding agent、EDA agent、内部自定义助手。

它们应该负责：

- 领域身份和 system prompt 内容。
- 领域专属工具。
- 领域专属 skills 和 prompt templates。
- 产品入口，例如 CLI、TUI、HTTP、RPC 或 MCP。
- 领域配置字段和默认策略。
- UI 渲染和产品工作流。

Domain agents 应复用 runtime 的共享应用行为，并在需要时直接使用 core 的 framework
基础能力。

## 4. Public API 分层

core 的 public API 应该按层级写清楚，让用户知道哪些接口稳定、哪些接口属于高级用法。

### 4.1 Stable SDK API

这些 API 会被 runtime 和 domain-agent 作者使用，应视为高稳定性的契约：

- `Agent`
- `AgentHarness`
- `AgentOptions`
- `AgentHarnessOptions`
- `Tool`、`ToolContext`、`ToolResult`、`ToolExecutionMode`
- `ExecutionEnv`、`ShellOptions`、文件/环境错误类型
- `AgentMessage`、`ContentBlock`、各类 message struct
- `AgentEvent`、`AgentHarnessEvent`
- `Session`、`SessionRepo`、`SessionStorage`
- public session API 所需的 session entry 和 metadata 类型
- `BuiltContext`
- `HarnessHooks` 和各类 hook trait
- `CompactionSettings`、`CompactionPreparation`、compaction result 类型
- `Skill`、`SourcedSkill`、`PromptTemplate`、diagnostics
- `OsEnv`

破坏这些契约时，应该同步更新设计文档、迁移说明和 changelog。

### 4.2 Advanced API

这些 API 面向 framework 作者和特殊集成场景公开，但不是大多数 SDK 用户的首选入口：

- `agent_loop`
- `agent_loop_continue`
- `LoopConfig`
- `RetryConfig`
- `HookedTool`
- `ConvertToLlmHook`
- `CustomMessageConverter`
- `DefaultConvertToLlm`

这些 API 可以比 stable tier 演进得更快，但 runtime 也会依赖其中一部分，因此变化时仍然
需要清晰的迁移说明。

### 4.3 Experimental / Implementation-Facing API

这些 API 不应该在 SDK 文档中主推：

- 直接 re-export 的 `llm_adapter` 类型。
- 低层 adapter request/response 类型。
- 用户不必要直接接触的内部 state snapshot。
- 私有 bridge 和 conversion helper。

如果用户经常需要这些类型，说明 core 应该提供一个更合适的自有抽象。

## 5. 推荐使用路径

SDK 文档应描述四条使用路径：

1. 轻量脚本或测试：使用 `Agent`。
2. 有 session、hooks、skills、compaction 的真实 agent：使用 `AgentHarness`。
3. framework 实验或自定义 runtime：使用 `agent_loop`。
4. 想要内置 tools、settings、auth、prompt assembly 的产品级 agent：使用
   `llm-harness-runtime`。

core 文档不应暗示 `coding-agent` 是当前 workspace 的一部分。

## 6. Core 与 Runtime 的集成契约

runtime 应通过明确的 core 契约集成：

- tools 以 `Vec<Arc<dyn Tool>>` 传给 core。
- runtime 可以用 `set_tools` 和 `set_active_tools` 管理工具可用性。
- runtime 可以实现或包装 `ExecutionEnv`，用于沙箱和权限策略。
- runtime 使用 `HarnessHooks` 在 provider 请求、context transform、tool call、
  compaction 等位置注入行为。
- runtime 订阅 `AgentHarnessEvent`，用于 UI、日志、telemetry 和 extension 事件分发。
- runtime 使用 `SessionRepo`，或提供自定义实现。
- runtime 使用 core 的 skills/templates 加载函数，但拥有更高层的资源发现和路径策略。
- runtime 使用 `CompactionSettings` 和 `AgentHarness::compact`，但拥有自动触发策略。
- runtime 只有在引入 core 默认无法转换的 custom message 时，才需要提供自定义
  `ConvertToLlmHook`。

core 应避免为 runtime 专属场景增加字段，除非该字段对独立 framework 用户也具有通用价值。

## 7. SDK 文档要求

现有设计 specs 对维护者有价值，但不足以作为用户向 SDK 文档。core 应提供以下用户文档：

- `Agent` quick start。
- `AgentHarness` guide，覆盖 session setup、tools、event subscription。
- Tool authoring guide。
- Hook lifecycle guide。
- Session model guide，覆盖 entry tree、branch 操作和 `build_context`。
- Skills/templates guide。
- Core/runtime/domain boundary guide。
- API stability guide。

README 必须修正为匹配当前 workspace：

- 从 workspace layout 中移除 `coding-agent`。
- 移除 `cargo run -p coding-agent` 等命令。
- 明确 concrete tools 位于 core 之外。
- 当 `llm-harness-runtime` 仓库可用后，引导需要 batteries-included agent runtime
  的用户使用 runtime。

## 8. 清理要求

当前仓库应该清理到与声明边界一致：

- 如果当前 crate 不再需要，应移除只为旧 tools 存在的 workspace dependencies，例如
  `globset`、`ignore`、`regex`。
- 现有 `Cargo.lock` git-source 问题作为独立依赖卫生任务处理。
- `docs/superpowers/plans/shared-agent-runtime-layer.md` 应视为初步分析，不作为权威边界
  spec。
- 本文作为 core SDK 与 runtime 的权威边界依据。

## 9. 稳定性规则

稳定性应基于 API tier：

- Stable SDK API：避免破坏性变更；无法避免时，需要设计文档更新和迁移说明。
- Advanced API：在 0.x 阶段可以随 minor release 演进，但必须文档化。
- Experimental API：不应出现在 examples 或 README 中。

Trait 稳定性尤其重要。`Tool`、`ExecutionEnv`、session traits、message types、
event types 和 hook traits 是 `llm-harness-runtime` 与 domain agents 的主要集成契约。

相比修改 required trait methods，优先选择增加带默认实现的 optional methods。

## 10. 验证要求

任何声称改善 SDK 边界的变更，应验证：

```powershell
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
```

文档示例只能引用当前 workspace 中存在的 crate，除非明确标注为未来
`llm-harness-runtime` 示例。

## 11. 非目标

本文不设计 `llm-harness-runtime` 的内部 API，只定义边界和集成契约。

本文不向 core 添加 concrete tools。

本文不设计任何 domain agent、CLI、TUI、HTTP API、MCP server 或 extension marketplace。
