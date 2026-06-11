# 快速开始

当你需要一个轻量、有状态，但不需要持久化 session、skills、compaction 或
branch 管理的 Agent 时，优先使用 `Agent`。这个入口适合测试、原型和简单脚本。

`Agent` 内部维护一份内存中的消息记录。它会发出 `AgentEvent`，并且使用和完整
harness 相同的 `Tool` 抽象。

## 最小形态

```rust
use std::sync::Arc;

use llm_harness::prelude::{Agent, AgentOptions};
use llm_harness_loop::LlmClient;

async fn run(client: Arc<dyn LlmClient>) -> anyhow::Result<()> {
    let mut opts = AgentOptions::new("my-model");
    opts.system_prompt = Some("You are a helpful assistant.".into());

    let agent = Agent::new(client, opts);
    agent.prompt("Summarize this repository.").await?;
    agent.wait_for_idle().await;

    Ok(())
}
```

具体如何构造 client 取决于 `llm-api-adapter`。Core 只接收
`Arc<dyn LlmClient>`，不负责 provider 凭证发现。没有工具的 Agent 不需要执行环境；
如果工具需要文件系统、shell、权限或 sandbox 策略，由 runtime 层实现
`ExecutionEnv` 后通过 `AgentOptions::new_with_env` 或 `AgentOptions::with_env` 注入。

如果想看一个真实 provider 的完整命令行示例，参考
`examples/deepseek-agent`。

## 什么时候改用 AgentHarness

当你需要持久化 session、branch 操作、compaction、skills/templates 或
harness hooks 时，使用 `AgentHarness`。如果要构建带 settings、auth、
model registry 和通用 tools 的产品型 Agent，应当在 core 之上的 runtime 层继续封装。
