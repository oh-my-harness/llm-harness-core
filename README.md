# llm-harness-core

Rust workspace for building LLM agent runtimes. The project separates pure agent
types, the streaming loop, stateful harness/session logic, and a small coding
agent CLI.

## Workspace Layout

```text
crates/
  llm-harness-types   Pure shared types and traits
  llm-harness-loop    Streaming agent loop and adapter bridge
  llm-harness         Agent, harness, sessions, compaction, skills
  coding-agent        CLI application built on the harness
```

The LLM provider layer is supplied by `llm_adapter` from:

```text
https://github.com/oh-my-harness/llm-api-adapter.git
```

The dependency is pinned by commit in `Cargo.toml` and `Cargo.lock` for
reproducible builds.

## Requirements

- Rust toolchain with edition 2024 support
- Network access for first-time Cargo dependency fetches
- `ANTHROPIC_API_KEY` when running the `coding-agent` CLI against Anthropic

On Windows, the shell tool expects a Bash-compatible shell. Resolution order:

1. `CODING_AGENT_SHELL`
2. `C:\Program Files\Git\bin\bash.exe`
3. `bash.exe` on `PATH`

The `grep` and `find` tools are implemented in Rust and do not require `rg`,
`grep`, `fd`, `sort`, or `head`.

## Build And Test

```powershell
cargo check
cargo build
cargo test
```

## Run The CLI

```powershell
$env:ANTHROPIC_API_KEY = "<your-key>"
cargo run -p coding-agent -- -p "summarize this repository"
```

Interactive mode:

```powershell
cargo run -p coding-agent -- --interactive
```

Session utilities:

```powershell
cargo run -p coding-agent -- --list-sessions
cargo run -p coding-agent -- --session-id <id> -p "continue"
cargo run -p coding-agent -- --delete-session <id>
```

## Line Endings

The repository uses `.gitattributes` to keep Rust, TOML, Markdown, and
`Cargo.lock` files on LF line endings. Recommended local Git settings:

```powershell
git config core.autocrlf false
git config core.eol lf
```
