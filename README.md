# llm-harness-core

用于构建 LLM Agent 框架的 Rust workspace。项目将共享 Agent 类型、流式
loop，以及带 session 的 harness 层分离开来。

`llm-harness-core` 是框架 SDK，不是具体的 Agent 产品。它定义 Agent
如何运行；具体工具、产品提示词、settings/auth 管理、CLI、TUI、HTTP、RPC、
MCP 和打包能力属于上层，例如 `llm-harness-runtime` 或具体业务 Agent 仓库。

## Workspace 结构

```text
crates/
  llm-harness-types   共享 messages、events、Tool、ExecutionEnv、hooks
  llm-harness-loop    流式 Agent loop 和 adapter bridge
  llm-harness         Agent、AgentHarness、sessions、compaction、skills
```

LLM provider 层由 `llm_adapter` 提供，来源：

```text
https://github.com/oh-my-harness/llm-api-adapter.git
```

该依赖在 `Cargo.toml` 和 `Cargo.lock` 中固定到具体 commit，以保证构建可复现。

## Core 包含什么

- Message 和 content model。
- Tool 和 execution environment 抽象。
- 流式 LLM loop 和 tool 调度。
- 用于轻量有状态运行的 `Agent`。
- 用于 session-backed Agent 的 `AgentHarness`。
- Session storage、branches、context rebuilding、compaction、skills 和 hooks。

## Core 不包含什么

- 具体工具，例如 read、bash、edit、write、grep、find 或 ls。
- Tool registry policy、settings、auth storage 或 model registry。
- 产品 system prompts。
- CLI、TUI、HTTP、RPC、MCP 或产品打包。
- 业务领域特定 tools 或 resources。

这些职责属于 `llm-harness-runtime` 或具体 Agent 仓库。

## 推荐使用路径

- 如果不需要持久化 session，用 `Agent` 构建轻量脚本、测试和原型。
- 如果需要 sessions、hooks、skills、compaction、events 和 branch 操作，用
  `AgentHarness` 构建真实 Agent。
- 只有在高级框架集成或自定义 runtime 场景中，才直接使用 `agent_loop`。
- 如果需要带通用 tools、settings、auth、model registry 和 prompt assembly 的
  应用 runtime，使用 `llm-harness-runtime`。

## 环境要求

- 支持 Rust 2024 edition 的 Rust toolchain。
- 首次拉取 Cargo 依赖时需要网络访问。

## 构建、测试和文档

```powershell
cargo check --workspace
cargo build --workspace
cargo test --workspace
cargo doc --workspace --no-deps
```

生成的 API 文档位于：

```text
target/doc/llm_harness/index.html
```

## 示例

运行 DeepSeek-backed `Agent` 持续对话示例：

```powershell
$env:DEEPSEEK_API_KEY="sk-..."
cargo run -p llm-harness --example deepseek_agent
```

可选配置：

```powershell
$env:DEEPSEEK_MODEL="deepseek-v4-flash"
$env:LLM_HARNESS_PROMPT="Say hello from llm-harness."
```

如果要测试推理模型，可以把 `DEEPSEEK_MODEL` 设为 `deepseek-reasoner`。

启动后输入消息并回车；输入 `exit` 或 `quit` 退出。

更多说明见
`crates/llm-harness/examples/deepseek_agent.md`。

## 设计文档

- SDK 指南：`docs/sdk/README.md`
- Core 设计：`docs/superpowers/specs/2026-06-07-llm-harness-core-design.md`
- Core/runtime SDK 边界：
  `docs/superpowers/specs/2026-06-10-core-runtime-sdk-boundary-design.md`
- 实现计划：
  `docs/superpowers/plans/2026-06-10-core-sdk-boundary.md`

## 换行符

仓库使用 `.gitattributes` 保持 Rust、TOML、Markdown 和 `Cargo.lock` 文件为
LF 换行。推荐的本地 Git 设置：

```powershell
git config core.autocrlf false
git config core.eol lf
```
