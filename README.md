# llm-harness-core

Rust workspace for building LLM agent frameworks. The project separates shared
agent types, the streaming loop, and the session-backed harness layer.

`llm-harness-core` is a framework SDK, not a concrete agent product. It defines
how agents run; concrete tools, product prompts, settings/auth management, CLI,
TUI, HTTP, RPC, MCP, and packaging live in upper layers such as
`llm-harness-runtime` or domain-agent repositories.

## Workspace Layout

```text
crates/
  llm-harness-types   Shared messages, events, Tool, ExecutionEnv, hooks
  llm-harness-loop    Streaming agent loop and adapter bridge
  llm-harness         Agent, AgentHarness, sessions, compaction, skills
```

The LLM provider layer is supplied by `llm_adapter` from:

```text
https://github.com/oh-my-harness/llm-api-adapter.git
```

The dependency is pinned by commit in `Cargo.toml` and `Cargo.lock` for
reproducible builds.

## What Belongs Here

- Message and content model.
- Tool and execution environment abstractions.
- Streaming LLM loop and tool scheduling.
- `Agent` for lightweight stateful runs.
- `AgentHarness` for session-backed agents.
- Session storage, branches, context rebuilding, compaction, skills, and hooks.

## What Does Not Belong Here

- Concrete tools such as read, bash, edit, write, grep, find, or ls.
- Tool registry policy, settings, auth storage, or model registry.
- Product system prompts.
- CLI, TUI, HTTP, RPC, MCP, or product packaging.
- Domain-specific tools or resources.

Those responsibilities belong in `llm-harness-runtime` or concrete agent
repositories.

## Recommended Usage Paths

- Use `Agent` for lightweight scripts, tests, and prototypes that do not need
  persistent sessions.
- Use `AgentHarness` for real agents that need sessions, hooks, skills,
  compaction, events, and branch operations.
- Use `agent_loop` directly only for advanced framework integrations or custom
  runtimes.
- Use `llm-harness-runtime` when you want a batteries-included application
  runtime with common tools, settings, auth, model registry, and prompt assembly.

## Requirements

- Rust toolchain with edition 2024 support.
- Network access for first-time Cargo dependency fetches.

## Build, Test, And Docs

```powershell
cargo check --workspace
cargo build --workspace
cargo test --workspace
cargo doc --workspace --no-deps
```

Generated API docs are written under:

```text
target/doc/llm_harness/index.html
```

## Design Documents

- SDK guides: `docs/sdk/README.md`
- Core design: `docs/superpowers/specs/2026-06-07-llm-harness-core-design.md`
- Core/runtime SDK boundary:
  `docs/superpowers/specs/2026-06-10-core-runtime-sdk-boundary-design.md`
- Implementation plan:
  `docs/superpowers/plans/2026-06-10-core-sdk-boundary.md`

## Line Endings

The repository uses `.gitattributes` to keep Rust, TOML, Markdown, and
`Cargo.lock` files on LF line endings. Recommended local Git settings:

```powershell
git config core.autocrlf false
git config core.eol lf
```
