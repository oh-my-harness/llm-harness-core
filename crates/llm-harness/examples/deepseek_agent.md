# DeepSeek Agent Example

这个示例展示如何用 `llm-harness` 的 `Agent` 连接真实 DeepSeek provider，并做一个
可以持续输入的命令行聊天循环。

它是一个最小聊天 Agent：

- 使用 `llm_adapter::deepseek::client` 创建 DeepSeek client。
- 使用 `AgentOptions::new(model)` 创建无工具 Agent。
- 持续读取命令行输入。
- 每次输入都复用同一个 `Agent`，因此会保留前文 transcript。
- 打印本轮 assistant 回复。
- 统计本轮收到的 text / thinking streaming event 数量。

它不是 coding agent，也不包含 read、bash、edit、search 等工具。因为没有工具，
所以不需要注入真实 `ExecutionEnv`。`AgentOptions::new(model)` 会使用内置的
`UnsupportedEnv`；只有当工具需要文件系统、shell、sandbox 或权限策略时，才需要
runtime 通过 `AgentOptions::new_with_env` 或 `AgentOptions::with_env` 注入环境。

## 运行

```powershell
$env:DEEPSEEK_API_KEY="sk-..."
cargo run -p llm-harness --example deepseek_agent
```

默认模型是 `deepseek-v4-flash`。

启动后输入消息并回车；输入 `exit` 或 `quit` 退出。

## 可选配置

```powershell
$env:DEEPSEEK_MODEL="deepseek-v4-flash"
$env:LLM_HARNESS_PROMPT="Say hello from llm-harness."
```

`LLM_HARNESS_PROMPT` 会作为启动后的第一轮输入；执行完后仍然会进入持续对话模式。

如果想测试推理模型，可以使用：

```powershell
$env:DEEPSEEK_MODEL="deepseek-reasoner"
```

## 这个示例展示的 SDK 路径

核心代码大致是：

```rust
let client = Arc::new(deepseek::client(api_key)) as Arc<dyn LlmClient>;
let mut opts = AgentOptions::new(model);
opts.system_prompt = Some("You are a concise assistant.".into());

let agent = Agent::new(client, opts);

loop {
    let prompt = read_user_input();
    agent.prompt(prompt).await?;
}
```

这就是没有工具时的最小持续对话方式。

## 后续扩展示例

如果要展示工具能力，应该新建单独示例，例如：

- `deepseek_with_calculator`
- `deepseek_with_runtime_env`
- `harness_with_session`

这样可以避免把 provider quick start 和工具/runtime 概念混在一起。
