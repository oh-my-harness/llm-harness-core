# AgentHarness 指南

当 Agent 需要持久化对话状态时，使用 `AgentHarness`。它会直接驱动
`agent_loop`，并不是对 `Agent` 的包装。Session storage 是消息历史的事实来源。

以下场景适合使用 `AgentHarness`：

- 带 session 的 prompt。
- branch、navigate 和 delete 操作。
- Compaction。
- Skills 和 prompt templates。
- Harness hooks。
- 面向 UI、日志或 telemetry 的事件订阅。

## 最小形态

```rust
use std::sync::Arc;

use llm_harness::{AgentHarness, AgentHarnessOptions, OsEnv};
use llm_harness_loop::LlmClient;

async fn run(client: Arc<dyn LlmClient>) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let env = Arc::new(OsEnv::new(cwd));

    let mut opts = AgentHarnessOptions::new("my-model");
    opts.system_prompt = Some("You are a helpful assistant.".into());

    let harness = AgentHarness::new_in_memory(client, env, opts).await;
    let mut events = harness.subscribe();

    harness.prompt("Start a session.").await?;
    harness.wait_for_idle().await;

    while let Ok(event) = events.try_recv() {
        println!("{event:?}");
    }

    Ok(())
}
```

## Tools

Core 不提供具体工具。可以通过 `AgentHarnessOptions` 传入
`Vec<Arc<dyn Tool>>`，或者调用 `set_tools` 注册工具。
如果希望注册一组较大的工具，但在某个 turn 中只暴露其中一部分，使用
`set_active_tools`。

## Sessions

测试和原型可以使用 `AgentHarness::new_in_memory`。如果 session 需要持久化，
使用 `JsonlSessionRepo` 配合 `AgentHarness::with_session`。
