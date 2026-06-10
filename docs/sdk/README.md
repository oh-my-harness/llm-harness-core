# llm-harness-core SDK 指南

这些文档说明如何把 `llm-harness-core` 作为 Agent 框架 SDK 使用。
它们面向 SDK 使用者，和 `docs/superpowers/specs/` 下更底层的设计 spec
互相补充。

- [快速开始](quick-start.md)：使用 `Agent` 构建轻量脚本和测试。
- [AgentHarness](agent-harness.md)：构建带 session 的 Agent。
- [Tool 编写](tool-authoring.md)：实现自定义工具。
- [Session 模型](session-model.md)：理解 entry、branch 和上下文构建。
- [Hooks](hooks.md)：定制 turn、tool 和 compaction 附近的行为。
- [Core / Runtime 边界](core-runtime-boundary.md)：判断哪些能力属于 core、
  runtime 或业务 Agent。
