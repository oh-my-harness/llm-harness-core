# llm-harness-core SDK 指南

这些文档说明如何把 `llm-harness-core` 作为 Agent 框架 SDK 使用。
它们面向 SDK 使用者，和 `docs/superpowers/specs/` 下更底层的设计 spec
互相补充。

## 从哪里开始

| 目标 | 推荐入口 | 先读 |
| --- | --- | --- |
| 构建轻量脚本、测试或无持久化聊天 | `Agent` | [快速开始](quick-start.md) |
| 接入真实 provider 跑一个 CLI 聊天 | `Agent` + provider client | [`examples/deepseek-agent`](../../examples/deepseek-agent/README.md) |
| 构建带 session、branch、compaction 的 Agent | `AgentHarness` | [AgentHarness](agent-harness.md) |
| 编写自定义工具 | `Tool` | [Tool 编写](tool-authoring.md) |
| 给工具提供文件系统、shell 或 sandbox 能力 | `ExecutionEnv` | [核心概念](concepts.md) |
| 判断能力应该放 core 还是 runtime | Core/runtime 边界 | [Core / Runtime 边界](core-runtime-boundary.md) |

## 文档索引

- [核心概念](concepts.md)：解释 `Agent`、`AgentHarness`、`Tool`、
  `ExecutionEnv`、`ToolContext`、events 和 provider client 的关系。
- [快速开始](quick-start.md)：使用 `Agent` 构建轻量脚本和测试。
- [AgentHarness](agent-harness.md)：构建带 session 的 Agent。
- [Tool 编写](tool-authoring.md)：实现自定义工具。
- [Session 模型](session-model.md)：理解 entry、branch 和上下文构建。
- [Hooks](hooks.md)：定制 turn、tool 和 compaction 附近的行为。
- [Core / Runtime 边界](core-runtime-boundary.md)：判断哪些能力属于 core、
  runtime 或业务 Agent。
