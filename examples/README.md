# Examples

这些示例展示如何把 `llm-harness-core` 当作 SDK 使用。示例放在根目录
`examples/` 下，并以独立 workspace package 的形式组织，这样更接近真实用户项目。

## 可用示例

| 示例 | 说明 | 运行 |
| --- | --- | --- |
| `deepseek-agent` | 使用真实 DeepSeek provider 的无工具命令行聊天 Agent。 | `cargo run -p deepseek-agent-example` |

## deepseek-agent

这个示例适合第一次验证 SDK 和 provider 接入是否工作。它展示：

- 如何创建 DeepSeek client。
- 如何用 `AgentOptions::new(model)` 创建无工具 Agent。
- 如何持续读取命令行输入并复用同一个 Agent transcript。
- 如何在运行期间订阅并统计事件。

运行：

```powershell
$env:DEEPSEEK_API_KEY="sk-..."
cargo run -p deepseek-agent-example
```

更多说明见 `examples/deepseek-agent/README.md`。

## 后续示例建议

后续可以继续补这些独立示例：

- `deepseek-with-tools`：展示工具注册、tool call 和 tool result。
- `deepseek-with-runtime-env`：展示需要文件系统或 shell 的工具如何使用
  `ExecutionEnv`。
- `harness-session-chat`：展示 `AgentHarness`、session、branch 和 compaction。
