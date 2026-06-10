# AgentHarness Guide

Use `AgentHarness` for real agents that need durable conversation state. It
directly drives `agent_loop`; it does not wrap `Agent`. Session storage is the
source of truth for message history.

`AgentHarness` is the right entrypoint for:

- Session-backed prompts.
- Branch, navigate, and delete operations.
- Compaction.
- Skills and prompt templates.
- Harness hooks.
- Event subscription for UI, logs, or telemetry.

## Minimal Shape

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

Core does not provide concrete tools. Register tools by passing
`Vec<Arc<dyn Tool>>` through `AgentHarnessOptions` or by calling `set_tools`.
Use `set_active_tools` when you want to register a broad tool set but expose
only a subset during a turn.

## Sessions

Use `AgentHarness::new_in_memory` for tests and prototypes. Use
`JsonlSessionRepo` plus `AgentHarness::with_session` when sessions should be
persisted.

