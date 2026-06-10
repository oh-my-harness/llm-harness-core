# Core SDK Boundary Implementation Plan

> **For agentic workers:** implement this plan task-by-task. Steps use checkbox
> (`- [ ]`) syntax for tracking. Keep commits scoped to the task groups below.

**Goal:** Align `llm-harness-core` with the approved SDK boundary spec:
`docs/superpowers/specs/2026-06-10-core-runtime-sdk-boundary-design.md`.

**Architecture:** `llm-harness-core` remains a complete agent framework SDK with
three crates: `llm-harness-types`, `llm-harness-loop`, and `llm-harness`.
Concrete tools, CLI/TUI entrypoints, settings/auth/model registry, and product
system prompts remain outside core.

**Non-goals:**
- Do not implement `llm-harness-runtime`.
- Do not add concrete tools back into core.
- Do not design or modify coding-agent / EDA-agent products.
- Do not perform unrelated API refactors.

---

## Current Gaps

| Area | Current issue | Target state |
|---|---|---|
| README | Still describes removed `coding-agent` crate and CLI commands | Describes core as agent framework SDK |
| Workspace deps | `globset`, `ignore`, `regex` remain from removed tools | Remove if unused by current crates |
| User docs | Existing docs are design specs, not SDK usage guides | Add concise SDK guides |
| API surface | Top-level exports exist but no recommended import layer | Add `llm_harness::prelude` for stable SDK API |
| Crate docs | Crate-level docs do not explain API tiers or intended users | Add concise crate-level documentation |

---

## File Map

| File / Directory | Responsibility |
|---|---|
| `README.md` | Main project positioning and quick commands |
| `Cargo.toml` | Workspace dependencies cleanup |
| `Cargo.lock` | Updated by Cargo after dependency cleanup |
| `docs/sdk/quick-start.md` | Minimal `Agent` usage |
| `docs/sdk/agent-harness.md` | `AgentHarness` usage and event/session model |
| `docs/sdk/tool-authoring.md` | Implementing `Tool` |
| `docs/sdk/session-model.md` | Session entries, branches, and `build_context` |
| `docs/sdk/hooks.md` | Hook lifecycle and capabilities |
| `docs/sdk/core-runtime-boundary.md` | User-facing boundary summary |
| `crates/llm-harness/src/lib.rs` | `prelude` and crate-level docs |
| `crates/llm-harness-types/src/lib.rs` | crate-level docs for stable contracts |
| `crates/llm-harness-loop/src/lib.rs` | crate-level docs for advanced loop API |

---

## Task 1: Update README for Core SDK Positioning

**Files:**
- Modify: `README.md`

- [ ] Remove `coding-agent` from the workspace layout.
- [ ] Remove `cargo run -p coding-agent` examples.
- [ ] Remove references to built-in grep/find/bash/read tools.
- [ ] State that concrete tools live outside core.
- [ ] Describe the three crates and their responsibilities.
- [ ] Describe recommended usage paths:
  - `Agent` for lightweight scripts/tests.
  - `AgentHarness` for session-backed agents.
  - `agent_loop` for advanced framework integrations.
  - future `llm-harness-runtime` for batteries-included agent runtime.
- [ ] Keep build/test/doc commands:

```powershell
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
```

**Verification:**

```powershell
rg -n "coding-agent|cargo run -p coding-agent|grep and find tools|CODING_AGENT_SHELL" README.md
```

Expected: no matches except intentional references to future/domain agents, if
any.

**Suggested commit:**

```text
docs: update README for core SDK boundary
```

---

## Task 2: Remove Obsolete Tool Dependencies

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] Confirm `globset`, `ignore`, and `regex` are not used by current crates:

```powershell
rg -n "globset|ignore::|use ignore|regex::|use regex" crates
```

- [ ] Remove unused workspace dependencies from root `Cargo.toml`.
- [ ] Run Cargo verification to update the lockfile:

```powershell
cargo check --workspace
```

- [ ] Inspect `Cargo.lock` diff. It should only reflect dependency cleanup and
  the existing `llm_adapter` git-source normalization.

**Verification:**

```powershell
cargo check --workspace
git diff --stat
```

**Suggested commit:**

```text
chore: remove obsolete tool dependencies
```

---

## Task 3: Add User-Facing Core SDK Guides

**Files:**
- Create: `docs/sdk/quick-start.md`
- Create: `docs/sdk/agent-harness.md`
- Create: `docs/sdk/tool-authoring.md`
- Create: `docs/sdk/session-model.md`
- Create: `docs/sdk/hooks.md`
- Create: `docs/sdk/core-runtime-boundary.md`
- Modify: `README.md`

Write concise guides, not full manuals. Each guide should include:

- What problem this API solves.
- When to use it.
- A minimal code sketch.
- Links to related APIs or design docs.

Guide requirements:

- [ ] `quick-start.md`: show the lightweight `Agent` path.
- [ ] `agent-harness.md`: explain when to use `AgentHarness`, session-backed
  state, events, and tools.
- [ ] `tool-authoring.md`: explain `Tool`, `ToolContext`, `ToolResult`,
  `parameters_schema`, and cancellation/update semantics.
- [ ] `session-model.md`: explain entry tree, active cursor, branches,
  compaction summaries, and `build_context`.
- [ ] `hooks.md`: list hook categories and whether each can observe, modify, or
  stop behavior.
- [ ] `core-runtime-boundary.md`: summarize core/runtime/domain responsibilities
  in user-facing language.
- [ ] README links to the new `docs/sdk/` guide index or individual files.

Do not include examples that require a non-existent `coding-agent` crate.
Future `llm-harness-runtime` examples must be explicitly labeled as future.

**Verification:**

```powershell
rg -n "cargo run -p coding-agent|crates/coding-agent|coding-agent CLI" README.md docs/sdk
```

Expected: no matches.

**Suggested commit:**

```text
docs: add core SDK usage guides
```

---

## Task 4: Add Stable SDK Prelude

**Files:**
- Modify: `crates/llm-harness/src/lib.rs`

Add:

```rust
/// Recommended imports for most `llm-harness` SDK users.
pub mod prelude {
    pub use crate::{
        Agent, AgentHarness, AgentHarnessEvent, AgentHarnessOptions, AgentOptions,
        BuiltContext, CompactionSettings, InMemorySessionRepo, JsonlSessionRepo,
        OsEnv, PromptTemplate, Session, SessionRepo, SessionStorage, Skill,
        SkillDiagnostic, SourcedSkill,
    };
    pub use llm_harness_types::{
        AgentEvent, AgentMessage, AssistantMessage, ContentBlock, ExecutionEnv,
        HarnessError, ShellOptions, ThinkingLevel, Tool, ToolContext,
        ToolError, ToolExecutionMode, ToolResult, UserMessage,
    };
}
```

Adjust the exact export list to match existing public names. Do not include:

- `LoopConfig`
- `agent_loop`
- direct `llm_adapter` types
- internal state structs unless required for common SDK usage

**Verification:**

```powershell
cargo check --workspace
cargo doc --workspace --no-deps
```

**Suggested commit:**

```text
feat: add llm_harness prelude
```

---

## Task 5: Add Crate-Level API Tier Notes

**Files:**
- Modify: `crates/llm-harness/src/lib.rs`
- Modify: `crates/llm-harness-types/src/lib.rs`
- Modify: `crates/llm-harness-loop/src/lib.rs`

- [ ] `llm-harness-types`: document stable contracts such as messages, events,
  `Tool`, `ExecutionEnv`, hooks, and errors.
- [ ] `llm-harness-loop`: document that this is advanced API for framework
  authors and custom runtimes.
- [ ] `llm-harness`: document that this is the primary core SDK facade,
  exposing `Agent`, `AgentHarness`, sessions, compaction, skills, and `prelude`.
- [ ] Link to the boundary spec and SDK docs where appropriate.

Keep docs concise. Avoid duplicating the full spec.

**Verification:**

```powershell
cargo doc --workspace --no-deps
```

**Suggested commit:**

```text
docs: add crate-level SDK API notes
```

---

## Final Verification

Run:

```powershell
cargo check --workspace
cargo test --workspace
cargo doc --workspace --no-deps
rg -n "cargo run -p coding-agent|crates/coding-agent|coding-agent CLI" README.md docs/sdk
```

Expected:

- Cargo commands pass.
- No README or SDK guide examples reference a non-existent workspace crate.
- `Cargo.lock` changes are explained by dependency cleanup.
- No concrete tool implementation is added to core.

---

## Final Review Checklist

- [ ] README matches current workspace members.
- [ ] `llm-harness-core` is described as a complete agent framework SDK.
- [ ] Runtime is described only as an upper layer, not as a dependency of core.
- [ ] Concrete tools remain outside core.
- [ ] Stable/advanced API tiers are documented.
- [ ] `prelude` contains only stable, common SDK imports.
- [ ] Tests and docs pass.
