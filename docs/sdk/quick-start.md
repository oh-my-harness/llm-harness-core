# Quick Start

Use `Agent` when you need a lightweight stateful agent without persistent
sessions, skills, compaction, or branch management. This path is useful for
tests, prototypes, and simple scripts.

`Agent` owns an in-memory message transcript. It emits `AgentEvent` values and
uses the same `Tool` abstraction as the full harness.

## Minimal Shape

```rust
use std::sync::Arc;

use llm_harness::{Agent, AgentOptions, OsEnv};
use llm_harness_loop::LlmClient;

async fn run(client: Arc<dyn LlmClient>) -> anyhow::Result<()> {
    let env = Arc::new(OsEnv::new(std::env::current_dir()?));
    let mut opts = AgentOptions::new("my-model", env);
    opts.system_prompt = Some("You are a helpful assistant.".into());

    let agent = Agent::new(client, opts);
    agent.prompt("Summarize this repository.").await?;
    agent.wait_for_idle().await;

    Ok(())
}
```

The exact client construction depends on `llm-api-adapter`. Core accepts an
`Arc<dyn LlmClient>` and does not own provider credential discovery.

## When To Use AgentHarness Instead

Use `AgentHarness` when you need durable sessions, branch operations,
compaction, skills/templates, or harness hooks. For product-like agents with
settings, auth, model registry, and common tools, build on a runtime layer above
core.
