# Core / Runtime Boundary

`llm-harness-core` is the framework SDK. It owns how agents run.

`llm-harness-runtime` is an upper layer. It should own shared product-runtime
behavior such as common tools, config, auth, model registry, prompt assembly,
auto retry, and auto compaction policy.

Domain agents own domain identity and product entrypoints.

## Core Owns

- Messages and content blocks.
- `Tool` and `ExecutionEnv` contracts.
- Streaming loop and tool scheduling.
- `Agent` and `AgentHarness`.
- Sessions, branches, context rebuilding.
- Compaction primitives.
- Skills and prompt templates.
- Hooks and events.

## Runtime Owns

- Concrete tools such as read, bash, edit, write, grep, find, and ls.
- Tool registry and active tool policy.
- Settings, auth, and model registry.
- System prompt builder.
- Resource discovery and orchestration.
- Retry policy and automatic compaction triggers.
- Extension/plugin runtime.

## Domain Agents Own

- Domain-specific tools.
- Domain-specific system prompt identity.
- Domain skills and templates.
- CLI, TUI, HTTP, RPC, MCP, and product workflows.

Core must not depend on runtime. Runtime depends on core.

