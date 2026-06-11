# 核心概念

这页用开发者视角解释 `llm-harness-core` 里的几个常见名词。它不替代 API
文档，而是帮助你先判断每个抽象该放在哪一层。

## Agent

`Agent` 是最轻量的入口。它维护一份内存中的消息记录，适合脚本、测试、原型和
不需要持久化 session 的小型应用。

如果你的应用只是：

- 构造一个 provider client。
- 设置 model 和 system prompt。
- 连续向模型提问。
- 可选地注册少量 tools。

优先从 `Agent` 开始。没有 tools 时，`AgentOptions::new(model)` 就够了，不需要
提供真实运行环境。

## AgentHarness

`AgentHarness` 面向更完整的 Agent 产品形态。它使用 session storage 作为消息历史
的事实来源，并支持 branch、navigate、compaction、skills、hooks 和更丰富的事件。

当你需要这些能力时，使用 `AgentHarness`：

- 对话需要持久化。
- 用户可以在 session branch 之间切换。
- 上下文可能超长，需要 compaction。
- 需要通过 hooks 介入 tool、turn 或 compaction 周期。
- UI、日志或 telemetry 需要订阅完整事件流。

`AgentHarness` 不是 `Agent` 的包装。它直接驱动底层 loop，并围绕 session 模型
组织状态。

## Tool

`Tool` 是模型请求外部动作的唯一入口。Core 不内置 read、bash、edit、search
这类具体工具；它只定义工具应该如何描述自己，以及如何被调用。

一个 tool 通常需要提供：

- `name`：稳定的工具名。
- `description`：给模型看的工具说明。
- `parameters_schema`：JSON schema，告诉模型应该传什么参数。
- `execute`：Rust 侧真正执行工具逻辑。

工具是否需要文件系统、shell、网络、权限或 sandbox，由工具自身决定。一个纯计算
工具可以完全不使用运行环境。

## ExecutionEnv

`ExecutionEnv` 是 runtime 提供给工具的运行环境抽象。它描述“工具如果需要操作外部
世界，应该通过谁来做”。

典型职责包括：

- 工作目录。
- 文件读写。
- shell 执行。
- 临时目录。
- 清理资源。
- 未来可能加入的权限或 sandbox 策略。

并不是所有 Agent 都需要真实 `ExecutionEnv`。无工具 Agent 或不依赖环境的工具可以
使用默认的 `UnsupportedEnv`。如果某个工具调用了环境能力，而 runtime 没有注入真实
环境，它会得到明确的不可用错误。

这也是 core/runtime 的边界：core 定义接口，runtime 决定具体 OS、权限和 sandbox
策略。

## ToolContext

`ToolContext` 是 core 在执行某个 tool 时传给它的上下文。它不是给 LLM 阅读的内容，
也不会自动进入 prompt。LLM 能看到的是 tool 的 name、description、schema，以及
工具执行后的结果。

`ToolContext` 给 Rust 工具代码使用，常见用途是：

- 通过 `ctx.env` 访问 runtime 提供的环境能力。
- 通过 `ctx.abort` 响应取消。
- 使用 `ctx.tool_use_id` 关联本次 tool call。
- 使用 `ctx.turn_index` 判断当前 turn。
- 查看触发本次工具调用的 assistant message。
- 对长时间运行的工具，通过 `update_tx` 输出增量结果。

如果工具不需要这些信息，可以忽略 `ToolContext`。

## Events

`AgentEvent` 和 `AgentHarnessEvent` 是运行过程中的事件流。它们适合驱动 UI、日志、
调试信息和 telemetry。

常见事件包括：

- Agent 或 turn 开始、结束。
- assistant message 开始、更新、结束。
- 文本或 thinking 的流式 delta。
- tool call 和 tool execution 的生命周期。
- 错误事件。

事件流是实时消费模型，不应该被当成无限历史记录。需要统计或展示流式内容时，应该在
运行期间订阅并消费事件。

## Provider Client

Core 不负责 provider 凭证发现，也不维护模型注册表。它只接收
`Arc<dyn LlmClient>`。具体 client 可以来自 `llm-api-adapter`，也可以由 runtime
或业务项目自己实现。

真实 provider 接入示例见 `examples/deepseek-agent`。

## API 稳定性

当前相对稳定、适合作为 SDK 入口使用的 API：

- `Agent` 和 `AgentOptions`。
- `AgentHarness` 和 `AgentHarnessOptions`。
- `Tool`、`ToolResult`、`ToolContext`。
- `ExecutionEnv`。
- message、content block、event 等核心类型。

仍然更偏框架内部或高级集成的 API：

- 直接调用 `agent_loop`。
- 自定义 context transform、retry、prepare-next-turn 等 loop 配置。
- 深度依赖 session 内部存储格式。

如果只是接入 SDK，优先从 `Agent`、`AgentHarness`、`Tool` 和 `ExecutionEnv`
这几个入口开始。
