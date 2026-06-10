# Hooks

Hooks 允许调用方在不修改 core loop 或 harness 逻辑的情况下定制行为。
它们按照运行时机以及是否能修改行为来分组。

## Harness Hooks

`HarnessHooks` 是 harness-level hooks 的统一配置对象。每次运行前，
harness 会根据 hooks 和当前状态构建一个临时的 `LoopConfig`。

常见 hook 分类：

- Run 和 turn hooks：观察或准备生命周期边界。
- Context hooks：在 messages 发送给 provider 前进行转换。
- Tool hooks：允许、修改、拒绝或 patch tool execution。
- Provider hooks：观察或定制 request-level 行为。
- Compaction hooks：决定或准备 compaction 行为。
- Stop hooks：决定 loop 是否继续。

## Events 与 Hooks

Events 是通知。当你需要 UI 更新、日志、telemetry 或进度渲染时，订阅
`AgentEvent` 或 `AgentHarnessEvent`。

Hooks 是行为控制点。当你需要修改 context、tool arguments、tool results 或运行决策时，
使用 hooks。

## Runtime 层

Runtime 或产品层可以把多个 plugins/extensions 汇聚到 core 的 hook set 中。
Core 只暴露类型化的 hook points，但不拥有 plugin runtime。
