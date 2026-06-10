# Core / Runtime 边界

`llm-harness-core` 是框架 SDK，负责 Agent 如何运行。

`llm-harness-runtime` 是上层。它应该负责共享的产品运行时行为，例如通用 tools、
config、auth、model registry、prompt assembly、auto retry 和 auto compaction policy。

业务 Agent 负责自己的领域身份和产品入口。

## Core 负责

- Messages 和 content blocks。
- `Tool` 和 `ExecutionEnv` contracts。
- Streaming loop 和 tool scheduling。
- `Agent` 和 `AgentHarness`。
- Sessions、branches 和 context rebuilding。
- Compaction primitives。
- Skills 和 prompt templates。
- Hooks 和 events。

## Runtime 负责

- 具体 tools，例如 read、bash、edit、write、grep、find 和 ls。
- Tool registry 和 active tool policy。
- Settings、auth 和 model registry。
- System prompt builder。
- Resource discovery 和 orchestration。
- Retry policy 和 automatic compaction triggers。
- Extension/plugin runtime。

## 业务 Agent 负责

- 领域专用 tools。
- 领域专用 system prompt identity。
- 领域 skills 和 templates。
- CLI、TUI、HTTP、RPC、MCP 和产品工作流。

Core 不应该依赖 runtime。Runtime 依赖 core。
