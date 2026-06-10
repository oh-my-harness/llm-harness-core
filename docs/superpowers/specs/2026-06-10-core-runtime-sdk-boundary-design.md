# llm-harness-core / runtime SDK Boundary Design

**Date:** 2026-06-10
**Status:** Proposed

## 1. Purpose

This document defines the SDK positioning and repository boundary between
`llm-harness-core`, the future `llm-harness-runtime`, and domain-specific agent
packages such as coding or EDA agents.

It does not replace the existing core architecture in
`2026-06-07-llm-harness-core-design.md`. Instead, it clarifies how that
architecture should be exposed as an SDK and where higher-level runtime
responsibilities belong.

## 2. Core Positioning

`llm-harness-core` is a complete agent framework SDK. It is not only a thin
kernel and it is not a product agent.

The existing three-crate architecture remains authoritative:

```text
llm-harness-types
  Shared messages, content blocks, events, Tool, ExecutionEnv, hooks, errors.

llm-harness-loop
  Streaming LLM loop, provider bridge, tool-call parsing, tool scheduling.

llm-harness
  Agent, AgentHarness, Session, Compaction, Skills/Templates, OsEnv.
```

The core SDK owns agent execution mechanics:

- Message and content model.
- Tool and environment abstractions.
- Streaming loop and tool scheduling.
- Agent events and harness events.
- Stateful `Agent`.
- Session-backed `AgentHarness`.
- Session storage/repository traits and bundled memory/JSONL implementations.
- Branching, navigation, and context rebuilding.
- Compaction primitives and orchestration hooks.
- Skills and prompt templates.
- Harness hooks.

The core SDK does not own product-level behavior:

- Concrete tools such as read, bash, edit, write, grep, find, or ls.
- Tool registry policy beyond accepting `Vec<Arc<dyn Tool>>` and active tool
  names.
- Settings, auth storage, model registry, or API-key discovery.
- Product system prompts.
- CLI, TUI, HTTP, RPC, MCP, or package distribution.
- Extension/plugin runtime.
- Domain-specific tools or resources.

## 3. Repository Boundary

The repository split should be understood as three layers:

```text
llm-harness-core
  "How an agent runs."
  Stable framework mechanisms and extension contracts.

llm-harness-runtime
  "How a reusable agent product runtime is assembled."
  Shared application runtime built on core.

domain agents
  "What this agent is."
  Product or domain packages built on runtime and/or core.
```

### 3.1 llm-harness-core

`llm-harness-core` is suitable for:

- Framework users who need direct control over tools, sessions, hooks, and
  message conversion.
- Runtime authors building a higher-level SDK.
- Advanced integrations that need custom `ExecutionEnv`, `SessionRepo`, or
  `ConvertToLlmHook`.
- Tests and prototypes that can use `Agent` directly.

Core should remain usable without `llm-harness-runtime`.

### 3.2 llm-harness-runtime

`llm-harness-runtime` is the recommended home for shared application runtime
features that are reusable across multiple domain agents but are too concrete
for core:

- Basic tools: read, bash, edit, write, grep, find, ls.
- Tool registry and tool selection policy.
- Tool prompt snippets and tool usage guidelines.
- System prompt builder framework.
- Settings loading and layered config.
- Auth storage and runtime auth resolution.
- Model registry and model selection helpers.
- Resource orchestration over skills, prompt templates, context files, and
  extension-provided resources.
- Automatic retry policy.
- Automatic compaction policy.
- Extension/plugin runtime that multiplexes many extensions onto core
  `HarnessHooks`.
- Higher-level builders such as `AgentRuntimeBuilder`.

Runtime depends on core. Core must not depend on runtime.

Runtime must not reimplement `AgentHarness`, session persistence, compaction
entry semantics, skills/templates parsing, or the streaming loop. Those are
core responsibilities.

### 3.3 Domain Agents

Domain agents are product or domain packages. Examples include coding agents,
EDA agents, and custom internal assistants.

They should own:

- Domain identity and system prompt content.
- Domain-specific tools.
- Domain-specific skills and prompt templates.
- Product entrypoints such as CLI, TUI, HTTP, RPC, or MCP.
- Domain config fields and default policies.
- UI rendering and product workflows.

Domain agents should reuse runtime for shared application behavior and core for
framework primitives.

## 4. Public API Tiers

Core public API should be documented in tiers so users know which surface is
stable and which surface is for advanced integration.

### 4.1 Stable SDK API

These APIs are expected to be used by runtime and domain-agent authors and
should be treated as high-stability contracts:

- `Agent`
- `AgentHarness`
- `AgentOptions`
- `AgentHarnessOptions`
- `Tool`, `ToolContext`, `ToolResult`, `ToolExecutionMode`
- `ExecutionEnv`, `ShellOptions`, file/environment error types
- `AgentMessage`, `ContentBlock`, message structs
- `AgentEvent`, `AgentHarnessEvent`
- `Session`, `SessionRepo`, `SessionStorage`
- Session entry and metadata types needed by public session APIs
- `BuiltContext`
- `HarnessHooks` and hook traits
- `CompactionSettings`, `CompactionPreparation`, compaction result types
- `Skill`, `SourcedSkill`, `PromptTemplate`, diagnostics
- `OsEnv`

Breaking these contracts should require a design update, migration notes, and a
changelog entry.

### 4.2 Advanced API

These APIs are public for framework authors and unusual integrations, but they
are not the recommended first entrypoint for most SDK users:

- `agent_loop`
- `agent_loop_continue`
- `LoopConfig`
- `RetryConfig`
- `HookedTool`
- `ConvertToLlmHook`
- `CustomMessageConverter`
- `DefaultConvertToLlm`

These APIs may evolve faster than the stable tier, but changes still need clear
migration guidance because runtime depends on some of them.

### 4.3 Experimental / Implementation-Facing API

These should not be promoted in SDK documentation:

- Direct `llm_adapter` re-exports.
- Low-level adapter request/response types.
- Internal state snapshots that are not necessary for users.
- Private bridge and conversion helpers.

If users need these types frequently, that is a signal that core needs a better
owned abstraction.

## 5. Recommended Usage Paths

The SDK documentation should describe four paths:

1. Lightweight script or test: use `Agent`.
2. Real agent with sessions, hooks, skills, and compaction: use `AgentHarness`.
3. Framework experiment or custom runtime: use `agent_loop`.
4. Product-like agent with built-in tools, settings, auth, and prompt assembly:
   use `llm-harness-runtime`.

Core documentation should not imply that `coding-agent` is part of this
workspace.

## 6. Core and Runtime Integration Contract

Runtime should integrate with core through explicit contracts:

- Tools are passed to core as `Vec<Arc<dyn Tool>>`.
- Runtime may use `set_tools` and `set_active_tools` to manage tool availability.
- Runtime implements or wraps `ExecutionEnv` for sandboxing and policy.
- Runtime uses `HarnessHooks` to inject behavior around provider requests,
  context transformation, tool calls, and compaction.
- Runtime subscribes to `AgentHarnessEvent` for UI, logging, telemetry, and
  extension event fan-out.
- Runtime uses `SessionRepo` or provides a custom implementation.
- Runtime uses core skills/templates loading functions but owns higher-level
  resource discovery and path policy.
- Runtime uses `CompactionSettings` and `AgentHarness::compact`, but owns the
  auto-trigger policy.
- Runtime may provide a custom `ConvertToLlmHook` only when it introduces custom
  messages that core cannot convert by default.

Core should avoid adding runtime-specific fields to these contracts unless the
same field is broadly useful for independent framework users.

## 7. SDK Documentation Requirements

The current design specs are valuable for maintainers but are not sufficient as
SDK documentation. Core should provide user-facing docs for:

- Quick start with `Agent`.
- `AgentHarness` guide with session setup, tools, and event subscription.
- Tool authoring guide.
- Hook lifecycle guide.
- Session model guide covering entry trees, branch operations, and
  `build_context`.
- Skills/templates guide.
- Core/runtime/domain boundary guide.
- API stability guide.

README must be corrected to match the current workspace:

- Remove `coding-agent` from the workspace layout.
- Remove commands such as `cargo run -p coding-agent`.
- State that concrete tools live outside core.
- Point users who want a batteries-included agent runtime to
  `llm-harness-runtime` once that repository exists.

## 8. Cleanup Requirements

The current repository should be cleaned so the declared boundary is reflected
in code and docs:

- Remove workspace dependencies that only existed for the removed tools, such as
  `globset`, `ignore`, and `regex`, unless another current crate needs them.
- Keep the existing `Cargo.lock` git-source issue as a separate dependency
  hygiene task.
- Treat `docs/superpowers/plans/shared-agent-runtime-layer.md` as preliminary
  analysis, not the authoritative boundary spec.
- Use this document as the authoritative boundary between core SDK and runtime.

## 9. Stability Rules

Stability should be based on API tier:

- Stable SDK API: avoid breaking changes; require design update and migration
  notes when unavoidable.
- Advanced API: may change with minor releases during the 0.x phase, but must
  be documented.
- Experimental API: should not be used in examples or README.

Trait stability is especially important. `Tool`, `ExecutionEnv`, session traits,
message types, event types, and hook traits are the main integration contracts
for `llm-harness-runtime` and domain agents.

Adding optional methods with default implementations is preferred over changing
required trait methods.

## 10. Validation

Changes that claim to improve the SDK boundary should verify:

```powershell
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
```

Documentation examples must reference only crates that exist in the current
workspace, unless explicitly labeled as future `llm-harness-runtime` examples.

## 11. Non-Goals

This spec does not design the internal API of `llm-harness-runtime`. It only
defines the boundary and integration contract.

This spec does not add concrete tools to core.

This spec does not design any domain agent, CLI, TUI, HTTP API, MCP server, or
extension marketplace.
