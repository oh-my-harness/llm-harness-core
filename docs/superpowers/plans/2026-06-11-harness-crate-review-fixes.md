# llm-harness Crate Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all P0, P1, and P2 issues found in the 2026-06-11 code review of `crates/llm-harness/src/`.

**Architecture:** Surgical per-issue fixes. TDD for all behavioral changes (write failing test → implement → pass). Doc comment tasks skip test steps. P0 fixes first (blocking), P1 next, P2 last.

**Tech Stack:** Rust, tokio, `llm-harness` crate, `llm-harness-types` crate, `llm-harness-loop` test-utils.

---

## File Map

| File | Changed by Tasks |
|------|-----------------|
| `crates/llm-harness/src/harness.rs` | 1, 2, 3, 4, 7, 8, 10, 11 |
| `crates/llm-harness/src/agent.rs` | 5, 6 |
| `crates/llm-harness/src/session/types.rs` | 3 |
| `crates/llm-harness/src/session/storage.rs` | 4, 9 |
| `crates/llm-harness/src/session/jsonl.rs` | 9 |
| `crates/llm-harness/src/session/repo.rs` | 4 |
| `crates/llm-harness/src/session/session.rs` | 4, 9 |
| `crates/llm-harness-types/src/errors.rs` | 8 |

---

## Task 1: Fix `flush_pending_writes` data-loss (P0-1a)

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Write the failing test**

Add inside the `#[cfg(test)] mod tests` block at the bottom of `harness.rs`, BEFORE `run_loop_phase_returns_idle_on_flush_error` (which doesn't exist yet):

First add the `FailAfterNStorage` test helper at the top of the `tests` module (after `use super::*;`):

```rust
use crate::session::storage::SessionStorage;
use crate::session::types::{SessionEntry, SessionMetadata, SessionEntryKind};
use futures::future::BoxFuture;
use std::sync::Arc;

/// Test helper: wraps any SessionStorage and fails `append_entry` after `fail_after` successes.
struct FailAfterNStorage {
    inner: Arc<dyn SessionStorage>,
    fail_after: usize,
    call_count: std::sync::Mutex<usize>,
}

impl FailAfterNStorage {
    fn new(inner: Arc<dyn SessionStorage>, fail_after: usize) -> Self {
        Self { inner, fail_after, call_count: std::sync::Mutex::new(0) }
    }
}

impl SessionStorage for FailAfterNStorage {
    fn append_entry(&self, entry: SessionEntry) -> BoxFuture<'_, Result<(), llm_harness_types::SessionError>> {
        Box::pin(async move {
            let mut n = self.call_count.lock().unwrap();
            if *n >= self.fail_after {
                return Err(llm_harness_types::SessionError::ConcurrentModification);
            }
            *n += 1;
            drop(n);
            self.inner.append_entry(entry).await
        })
    }
    fn metadata(&self) -> BoxFuture<'_, Result<SessionMetadata, llm_harness_types::SessionError>> { self.inner.metadata() }
    fn create_entry_id(&self) -> llm_harness_types::EntryId { self.inner.create_entry_id() }
    fn get_entry(&self, id: llm_harness_types::EntryId) -> BoxFuture<'_, Result<Option<SessionEntry>, llm_harness_types::SessionError>> { self.inner.get_entry(id) }
    fn children(&self, parent: llm_harness_types::EntryId) -> BoxFuture<'_, Result<Vec<SessionEntry>, llm_harness_types::SessionError>> { self.inner.children(parent) }
    fn all_leaves(&self) -> BoxFuture<'_, Result<Vec<llm_harness_types::EntryId>, llm_harness_types::SessionError>> { self.inner.all_leaves() }
    fn active_cursor(&self) -> BoxFuture<'_, Result<Option<llm_harness_types::EntryId>, llm_harness_types::SessionError>> { self.inner.active_cursor() }
    fn set_active_cursor(&self, id: llm_harness_types::EntryId) -> BoxFuture<'_, Result<(), llm_harness_types::SessionError>> { self.inner.set_active_cursor(id) }
    fn path_to_root(&self, target: llm_harness_types::EntryId) -> BoxFuture<'_, Result<Vec<SessionEntry>, llm_harness_types::SessionError>> { self.inner.path_to_root(target) }
    fn common_ancestor(&self, a: llm_harness_types::EntryId, b: llm_harness_types::EntryId) -> BoxFuture<'_, Result<Option<llm_harness_types::EntryId>, llm_harness_types::SessionError>> { self.inner.common_ancestor(a, b) }
    fn label_at(&self, id: llm_harness_types::EntryId) -> BoxFuture<'_, Result<Option<String>, llm_harness_types::SessionError>> { self.inner.label_at(id) }
    fn find_entries_by_type(&self, kind: SessionEntryKind) -> BoxFuture<'_, Result<Vec<llm_harness_types::EntryId>, llm_harness_types::SessionError>> { self.inner.find_entries_by_type(kind) }
    fn update_metadata_name(&self, name: Option<String>) -> BoxFuture<'_, Result<(), llm_harness_types::SessionError>> { self.inner.update_metadata_name(name) }
    fn update_metadata_model(&self, model: Option<String>) -> BoxFuture<'_, Result<(), llm_harness_types::SessionError>> { self.inner.update_metadata_model(model) }
    fn delete_entries(&self, ids: Vec<llm_harness_types::EntryId>) -> BoxFuture<'_, Result<(), llm_harness_types::SessionError>> { self.inner.delete_entries(ids) }
}

fn make_harness_failing_storage(fail_after: usize, responses: Vec<MockResponse>) -> AgentHarness {
    let repo = crate::session::repo::InMemorySessionRepo::new();
    let storage = futures::executor::block_on(
        repo.create(crate::session::types::CreateSessionOptions::default())
    ).unwrap();
    let failing = Arc::new(FailAfterNStorage::new(storage, fail_after));
    let session = crate::session::session::Session::new(failing as Arc<dyn SessionStorage>);
    let client = Arc::new(MockLlmClient::new(responses));
    let env = Arc::new(NoOpEnv);
    AgentHarness::with_session(client as Arc<dyn LlmClient>, env, session, AgentHarnessOptions::new("test-model"))
}
```

Then add the actual test:

```rust
#[tokio::test]
async fn flush_error_returns_phase_to_idle() {
    // Storage fails immediately on append_entry (fail_after=0)
    let h = make_harness_failing_storage(0, vec![MockResponse::text("hi")]);

    let result = h.prompt("hello").await;

    // Bug: phase stays Turning. Fix: phase must return to Idle.
    assert_eq!(h.state().phase, HarnessPhase::Idle, "phase must be Idle after flush error");
    assert!(result.is_err(), "prompt must return Err when storage fails");
}
```

- [ ] **Step 2: Verify the test fails (Red)**

Run:
```bash
cargo test --package llm-harness --features test-utils flush_error_returns_phase_to_idle
```
Expected: FAIL. The current code leaves phase as `Turning` after a storage error.

- [ ] **Step 3: Fix `flush_pending_writes` — peek, write, then remove**

Replace the current `flush_pending_writes` method (around line 1046 in `harness.rs`) with:

```rust
async fn flush_pending_writes(&self) -> Result<usize, HarnessError> {
    let mut count = 0;
    loop {
        let payload = {
            let inner = self.inner.lock().unwrap();
            inner.state.pending_session_writes.first().cloned()
        };
        let Some(payload) = payload else { break };
        // Write before removing. If this fails, payload remains in pending.
        // The run_loop cleanup in the Err branch will clear the remainder.
        self.session.append(payload).await?;
        self.inner
            .lock()
            .unwrap()
            .state
            .pending_session_writes
            .remove(0);
        count += 1;
    }
    Ok(count)
}
```

- [ ] **Step 4: Add cleanup block to `run_loop`**

The cleanup code currently at the end of `run_loop` (lines ~1347–1353) only runs on success. Wrap the loop body in an inner function and add a mandatory cleanup:

Replace `run_loop` entirely with two functions — `run_loop` (cleanup owner) and `drive_loop` (the inner loop logic):

```rust
async fn run_loop(&self, initial: Vec<AgentMessage>) -> Result<(), HarnessError> {
    // Setup: channels, abort token, system prompt.
    let (steer_rx, follow_up_rx, abort, system_prompt) = {
        let mut inner = self.inner.lock().unwrap();
        inner.state.streaming_message = None;
        inner.state.pending_tool_calls.clear();
        let (steer_rx, follow_up_rx) = inner.reset_channels();
        let abort = CancellationToken::new();
        inner.current_abort = Some(abort.clone());
        let sp = inner.state.system_prompt.clone();
        (steer_rx, follow_up_rx, abort, sp)
    };
    self.set_phase(HarnessPhase::Turning);

    let result = self
        .drive_loop(initial, steer_rx, follow_up_rx, abort, system_prompt)
        .await;

    // Cleanup always runs, success or failure.
    {
        let mut inner = self.inner.lock().unwrap();
        inner.state.streaming_message = None;
        inner.state.pending_tool_calls.clear();
        inner.current_abort = None;
        if result.is_err() {
            inner.state.error_message = Some(result.as_ref().unwrap_err().to_string());
            inner.state.pending_session_writes.clear();
        }
    }
    self.set_phase(HarnessPhase::Idle);
    result
}
```

Then rename the existing `run_loop` body (from `build_context` through the `while let Some(event)` loop, i.e., lines 1193–1353 of the old function) to `drive_loop`:

```rust
async fn drive_loop(
    &self,
    initial: Vec<AgentMessage>,
    steer_rx: mpsc::Receiver<AgentMessage>,
    follow_up_rx: mpsc::Receiver<AgentMessage>,
    abort: CancellationToken,
    system_prompt: Option<String>,
) -> Result<(), HarnessError> {
    // 2. Build context from session.
    let built = self.session.build_context().await?;
    // ... (rest of the current run_loop body, minus the setup block and cleanup block)
}
```

Remove the old `// 5. Clear running state and restore Idle.` cleanup block from the end of `drive_loop` (it now lives in `run_loop`).

- [ ] **Step 5: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass, including `flush_error_returns_phase_to_idle`.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/harness.rs
git commit -m "fix(harness): guarantee run_loop cleanup on error path (P0-1)"
```

---

## Task 2: Fix `ToolCallEnd.tool_name` hardcoded empty string (P0-2)

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `harness.rs`:

```rust
#[tokio::test]
async fn tool_call_end_event_carries_tool_name() {
    use std::sync::Mutex as StdMutex;

    let h = make_harness(vec![
        MockResponse::tool_use("id-1", "my_tool", "{}"),
        MockResponse::text("done"),
    ]);

    // Register a dummy tool so the loop can execute it
    let tool = Arc::new(crate::DummyTool::new("my_tool")); // see note below
    h.set_tools(vec![tool]).await.unwrap();

    let received_name: Arc<StdMutex<Option<String>>> = Arc::new(StdMutex::new(None));
    let received_name_clone = received_name.clone();

    let mut rx = h.subscribe();
    let handle = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let AgentHarnessEvent::ToolCallEnd { tool_name, .. } = event.as_ref() {
                *received_name_clone.lock().unwrap() = Some(tool_name.clone());
                break;
            }
        }
    });

    h.prompt("use tool").await.unwrap();
    handle.await.unwrap();

    assert_eq!(
        received_name.lock().unwrap().as_deref(),
        Some("my_tool"),
        "ToolCallEnd must carry the correct tool_name"
    );
}
```

Note: if a `DummyTool` helper doesn't exist in the test module, add one:
```rust
struct DummyTool { name: String }
impl DummyTool {
    fn new(name: &str) -> Self { Self { name: name.to_owned() } }
}
#[async_trait::async_trait]
impl llm_harness_types::Tool for DummyTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { "test tool" }
    fn input_schema(&self) -> serde_json::Value { serde_json::json!({"type":"object","properties":{}}) }
    async fn execute(&self, _args: serde_json::Value, _ctx: llm_harness_types::ToolContext) -> Result<llm_harness_types::ToolResult, llm_harness_types::ToolError> {
        Ok(llm_harness_types::ToolResult::text("ok"))
    }
    fn execution_mode(&self) -> llm_harness_types::ToolExecutionMode { llm_harness_types::ToolExecutionMode::Parallel }
}
```

- [ ] **Step 2: Verify the test fails (Red)**

```bash
cargo test --package llm-harness --features test-utils tool_call_end_event_carries_tool_name
```
Expected: FAIL with `ToolCallEnd.tool_name == ""` instead of `"my_tool"`.

- [ ] **Step 3: Add `active_tool_names` to `HarnessInner`**

In the `HarnessInner` struct (around line 206 in `harness.rs`), add one field:

```rust
/// Maps tool_use_id → tool_name for in-flight tool calls.
active_tool_names: std::collections::HashMap<String, String>,
```

Initialize it in `AgentHarness::with_session` (inside the `HarnessInner { ... }` literal):

```rust
active_tool_names: std::collections::HashMap::new(),
```

- [ ] **Step 4: Insert on start, remove+use on end**

In the `ToolExecutionStart` match arm (around line 1251 in `harness.rs`), after `pending_tool_calls.insert(...)`, add:

```rust
inner.active_tool_names.insert(tool_use_id.clone(), tool_name.clone());
```

In the `ToolExecutionEnd` match arm (around line 1269), when building the `ToolCallEnd` event, replace:

```rust
self.emit(AgentHarnessEvent::ToolCallEnd {
    tool_use_id: tool_use_id.clone(),
    tool_name: String::new(), // name not in ToolExecutionEnd
    ...
```

with:

```rust
let resolved_name = self
    .inner
    .lock()
    .unwrap()
    .active_tool_names
    .remove(tool_use_id.as_str())
    .unwrap_or_default();

self.emit(AgentHarnessEvent::ToolCallEnd {
    tool_use_id: tool_use_id.clone(),
    tool_name: resolved_name,
    ...
```

Also clear `active_tool_names` in the `drive_loop` cleanup (now in `run_loop`'s cleanup block):

```rust
inner.active_tool_names.clear();
```

- [ ] **Step 5: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/harness.rs
git commit -m "fix(harness): populate tool_name in ToolCallEnd event (P0-2)"
```

---

## Task 3: Add missing doc comments to `session/types.rs` (P0-3 to P0-8)

**Files:**
- Modify: `crates/llm-harness/src/session/types.rs`

No tests needed — `cargo doc --no-deps` will surface remaining gaps; compilation confirms syntax.

- [ ] **Step 1: Add `///` to `SessionEntryPayload` named-variant fields**

For each named-field variant in `SessionEntryPayload` (lines 10–49), add inline `///` above each field.  Example for `ModelChange`:

```rust
/// Model configuration change.
ModelChange {
    /// New model identifier.
    to: String,
    /// Optional provider override.
    provider: Option<String>,
    /// Optional raw model ID as returned by the provider.
    model_id: Option<String>,
},
```

Apply the same pattern to all named-field variants: `Label`, `SessionInfo`, `Custom` (fields `custom_type`, `data`), `BranchPoint` (fields `from`, `label`), `BranchSwitch` (fields `from`, `to`, `summary`).

- [ ] **Step 2: Add `///` to `CreateSessionOptions` and `ListSessionOptions` fields**

```rust
pub struct CreateSessionOptions {
    /// Optional human-readable session name.
    pub name: Option<String>,
    /// Initial model ID; stored as first `ModelChange` entry.
    pub initial_model: Option<String>,
    /// Initial thinking level.
    pub initial_thinking_level: Option<ThinkingLevel>,
    /// Initial active tool name list.
    pub initial_tools: Vec<String>,
}

pub struct ListSessionOptions {
    /// Max number of results to return.
    pub limit: Option<usize>,
    /// Zero-based page offset.
    pub offset: Option<usize>,
    /// Sort order.
    pub order: ListOrder,
    /// Filter to sessions whose name contains this substring.
    pub name_contains: Option<String>,
}
```

- [ ] **Step 3: Add `///` to `ListOrder` variants and `ForkOptions.name`**

```rust
pub enum ListOrder {
    /// Sort by `updated_at` descending (most recently modified first).
    #[default]
    UpdatedDesc,
    /// Sort by `updated_at` ascending (oldest modification first).
    UpdatedAsc,
    /// Sort by `created_at` descending (newest first).
    CreatedDesc,
    /// Sort by `created_at` ascending (oldest first).
    CreatedAsc,
}

pub struct ForkOptions {
    /// Optional name for the new forked session.
    pub name: Option<String>,
    ...
}
```

- [ ] **Step 4: Add `///` to `SessionEntryKind` variants**

```rust
pub enum SessionEntryKind {
    /// `SessionEntryPayload::Message` variant.
    Message,
    /// `SessionEntryPayload::ModelChange` variant.
    ModelChange,
    /// `SessionEntryPayload::ThinkingLevelChange` variant.
    ThinkingLevelChange,
    /// `SessionEntryPayload::ActiveToolsChange` variant.
    ActiveToolsChange,
    /// `SessionEntryPayload::Compaction` variant.
    Compaction,
    /// `SessionEntryPayload::Label` variant.
    Label,
    /// `SessionEntryPayload::SessionInfo` variant.
    SessionInfo,
    /// `SessionEntryPayload::Custom` variant.
    Custom,
    /// `SessionEntryPayload::BranchPoint` variant.
    BranchPoint,
    /// `SessionEntryPayload::BranchSwitch` variant.
    BranchSwitch,
    /// `SessionEntryPayload::BranchSummary` variant.
    BranchSummary,
}
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --package llm-harness
```
Expected: compiles without warnings about missing doc comments.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/session/types.rs
git commit -m "docs(session): add missing pub doc comments to types.rs (P0-3 to P0-8)"
```

---

## Task 4: Add missing doc comments to harness.rs, session/repo.rs, storage.rs, session.rs (P0-9 to P0-13)

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`
- Modify: `crates/llm-harness/src/session/repo.rs`
- Modify: `crates/llm-harness/src/session/storage.rs`
- Modify: `crates/llm-harness/src/session/session.rs`

- [ ] **Step 1: Add `///` to `AgentHarnessEvent` struct-variant fields (harness.rs ~130–202)**

For every struct variant that lacks field-level docs, add `///` above each field. Example for `PhaseChange`:

```rust
/// Harness phase changed.
PhaseChange {
    /// Phase before the transition.
    from: HarnessPhase,
    /// Phase after the transition.
    to: HarnessPhase,
},
```

Apply same pattern to: `ModelUpdate` (`from`, `to`), `ThinkingLevelUpdate` (`from`, `to`), `ToolsUpdate` (`added`, `removed`), `ActiveToolsUpdate` (`active`), `ResourcesUpdate` (`skills`, `templates`, `diagnostics`), `SessionInfoUpdate` (`name`), `CompactionStart` (`estimated_tokens`), `CompactionEnd` (`stats`, `error`), `QueueUpdate` (`steer_len`, `follow_up_len`), `SavePoint` (`entries_flushed`), `BranchForked` (`from`, `new_leaf`, `label`), `BranchSwitched` (`from`, `to`), `BranchDeleted` (`leaf`), `BranchSummarized` (`leaf`, `summary`), `ToolCallStart` (`tool_use_id`, `tool_name`, `args`), `ToolCallEnd` (`tool_use_id`, `tool_name`, `result`).

- [ ] **Step 2: Add `///` to `SessionRepo` trait methods (session/repo.rs ~17–37)**

```rust
pub trait SessionRepo: Send + Sync {
    /// Create a new session with the given options.
    fn create(&self, opts: CreateSessionOptions) -> BoxFuture<'_, Result<Arc<dyn SessionStorage>, SessionError>>;

    /// Open an existing session by ID.
    fn open(&self, id: &str) -> BoxFuture<'_, Result<Arc<dyn SessionStorage>, SessionError>>;

    /// List sessions matching `opts`.
    fn list(&self, opts: ListSessionOptions) -> BoxFuture<'_, Result<Vec<SessionMetadata>, SessionError>>;

    /// Permanently delete the session with the given ID.
    fn delete(&self, id: &str) -> BoxFuture<'_, Result<(), SessionError>>;
    ...
}
```

Add `/// Create a new in-memory session repository.` above `InMemorySessionRepo::new`.

- [ ] **Step 3: Add `///` to `SessionStorage` trait methods (session/storage.rs ~16–65)**

Add `///` to the 11 methods that currently lack it: `metadata`, `append_entry`, `get_entry`, `children`, `all_leaves`, `active_cursor`, `set_active_cursor`, `common_ancestor`, `find_entries_by_type`, `update_metadata_name`, `update_metadata_model`.

Example:
```rust
/// Return the current session metadata snapshot.
fn metadata(&self) -> BoxFuture<'_, Result<SessionMetadata, SessionError>>;

/// Append a pre-built entry; advances `active_cursor` to the new entry's ID.
fn append_entry(&self, entry: SessionEntry) -> BoxFuture<'_, Result<(), SessionError>>;
```

Add `/// Create a new in-memory session storage with the given initial metadata.` above `InMemorySessionStorage::new`.

- [ ] **Step 4: Add `///` to `Session::new` (session/session.rs line 19)**

```rust
/// Wrap a storage backend in the high-level `Session` interface.
pub fn new(storage: Arc<dyn SessionStorage>) -> Self {
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --package llm-harness
```
Expected: compiles without errors.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/harness.rs crates/llm-harness/src/session/repo.rs crates/llm-harness/src/session/storage.rs crates/llm-harness/src/session/session.rs
git commit -m "docs(harness): add missing pub doc comments (P0-9 to P0-13)"
```

---

## Task 5: Fix `clear_steering_queue` / `clear_follow_up_queue` (P1-1)

**Files:**
- Modify: `crates/llm-harness/src/agent.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` block in `agent.rs`:

```rust
#[tokio::test]
async fn clear_steering_queue_drains_pending_messages() {
    let agent = make_agent(vec![MockResponse::text("response")]);

    // Enqueue a steer message while Idle
    agent.steer_message("ignored message".to_owned());

    // Clear before prompting
    agent.clear_steering_queue();

    // Prompt with a fresh message
    agent.prompt("actual prompt").await.unwrap();

    let state = agent.state();
    // Messages: [user("actual prompt"), assistant("response")] = 2
    // If the queue was NOT cleared, the steer message would be in there too
    assert_eq!(state.messages.len(), 2, "steer message should have been cleared");
}
```

- [ ] **Step 2: Verify the test fails (Red)**

```bash
cargo test --package llm-harness --features test-utils clear_steering_queue_drains
```
Expected: FAIL — the steer message is not cleared by the no-op implementation.

- [ ] **Step 3: Implement `clear_steering_queue`**

Replace the empty body in `agent.rs` (around line 287):

```rust
pub fn clear_steering_queue(&self) {
    let mut inner = self.inner.lock().unwrap();
    let cap = inner.queue_capacity;
    let (new_tx, _) = mpsc::channel(cap);
    inner.steer_tx = new_tx;
    // Old receiver (if any, held by a running loop) is left intact.
    // Old sender is dropped; new channel starts empty.
}
```

- [ ] **Step 4: Implement `clear_follow_up_queue`**

Replace the empty `clear_follow_up_queue` body (around line 297):

```rust
pub fn clear_follow_up_queue(&self) {
    let mut inner = self.inner.lock().unwrap();
    let cap = inner.queue_capacity;
    let (new_tx, _) = mpsc::channel(cap);
    inner.follow_up_tx = new_tx;
}
```

- [ ] **Step 5: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/agent.rs
git commit -m "fix(agent): implement clear_steering_queue and clear_follow_up_queue (P1-1)"
```

---

## Task 6: Fix `prompt_with_messages` single-user-message heuristic (P1-2)

**Files:**
- Modify: `crates/llm-harness/src/agent.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `agent.rs`:

```rust
#[tokio::test]
async fn prompt_with_messages_single_user_replaces_transcript() {
    let agent = make_agent(vec![
        MockResponse::text("first response"),
        MockResponse::text("second response"),
    ]);

    // Build up an existing transcript
    agent.prompt("first prompt").await.unwrap();
    let before_len = agent.state().messages.len(); // user + assistant = 2

    // Call prompt_with_messages with a SINGLE user message (the heuristic path)
    let single_user = vec![AgentMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text: "clean start".into() }],
        timestamp: chrono::Utc::now(),
    })];
    agent.prompt_with_messages(single_user).await.unwrap();

    let state = agent.state();
    // Correct: transcript replaced → 1 user + 1 assistant from loop = 2 total
    // Bug: transcript appended → before_len + 2 messages
    assert_eq!(
        state.messages.len(),
        2,
        "single-user prompt_with_messages must replace transcript (was {before_len} before)"
    );
}
```

- [ ] **Step 2: Verify the test fails (Red)**

```bash
cargo test --package llm-harness --features test-utils prompt_with_messages_single_user_replaces
```
Expected: FAIL — current heuristic appends instead of replacing.

- [ ] **Step 3: Add `replace_transcript: bool` parameter to `run_with_initial`**

Change the signature from:
```rust
async fn run_with_initial(&self, initial: Vec<AgentMessage>, is_continue: bool) -> Result<(), AgentError>
```
to:
```rust
async fn run_with_initial(
    &self,
    initial: Vec<AgentMessage>,
    is_continue: bool,
    replace_transcript: bool,
) -> Result<(), AgentError>
```

- [ ] **Step 4: Replace the heuristic with the explicit flag**

In `run_with_initial`, replace the heuristic block (around lines 369–381):

```rust
// Old heuristic (REMOVE):
if initial.len() == 1 && matches!(initial[0], AgentMessage::User(_)) {
    inner.state.messages.extend(initial.iter().cloned());
} else {
    inner.state.messages = initial.clone();
}

// New explicit logic (ADD):
if replace_transcript {
    inner.state.messages = initial.clone();
} else if !initial.is_empty() {
    inner.state.messages.extend(initial.iter().cloned());
}
```

Update all callers of `run_with_initial`:

```rust
// prompt(): append one user message
self.run_with_initial(vec![user_msg], false, false).await

// prompt_with_messages(): replace transcript
self.run_with_initial(messages, false, true).await

// continue_run(): no messages, no replace
self.run_with_initial(vec![], true, false).await
```

- [ ] **Step 5: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass, including the existing `prompt_with_messages_replaces_transcript` (multi-message case) and the new single-message test.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness/src/agent.rs
git commit -m "fix(agent): replace_transcript flag eliminates prompt_with_messages heuristic (P1-2)"
```

---

## Task 7: Add phase check to `append_message` / `append_custom_entry` (P1-3)

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `harness.rs`:

```rust
#[tokio::test]
async fn append_message_while_turning_returns_not_idle() {
    let h = make_harness(vec![]);
    // Force phase to Turning
    h.inner.lock().unwrap().state.phase = HarnessPhase::Turning;

    let msg = AgentMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text: "test".into() }],
        timestamp: chrono::Utc::now(),
    });
    let err = h.append_message(msg).await;

    h.inner.lock().unwrap().state.phase = HarnessPhase::Idle; // cleanup
    assert!(
        matches!(err, Err(HarnessError::NotIdle(_))),
        "append_message must refuse while Turning"
    );
}

#[tokio::test]
async fn append_custom_entry_while_turning_returns_not_idle() {
    let h = make_harness(vec![]);
    h.inner.lock().unwrap().state.phase = HarnessPhase::Turning;

    let err = h
        .append_custom_entry("test_type".into(), serde_json::json!({}))
        .await;

    h.inner.lock().unwrap().state.phase = HarnessPhase::Idle;
    assert!(
        matches!(err, Err(HarnessError::NotIdle(_))),
        "append_custom_entry must refuse while Turning"
    );
}
```

- [ ] **Step 2: Verify the tests fail (Red)**

```bash
cargo test --package llm-harness --features test-utils append_message_while_turning
cargo test --package llm-harness --features test-utils append_custom_entry_while_turning
```
Expected: both FAIL — no phase check exists yet.

- [ ] **Step 3: Add phase guard to both methods**

Replace `append_message` (around line 719) with:

```rust
pub async fn append_message(&self, msg: AgentMessage) -> Result<EntryId, HarnessError> {
    {
        let inner = self.inner.lock().unwrap();
        if inner.state.phase != HarnessPhase::Idle {
            return Err(HarnessError::NotIdle(inner.state.phase));
        }
    }
    Ok(self.session.append_message(msg).await?)
}
```

Replace `append_custom_entry` (around line 723) with:

```rust
pub async fn append_custom_entry(
    &self,
    custom_type: String,
    data: serde_json::Value,
) -> Result<EntryId, HarnessError> {
    {
        let inner = self.inner.lock().unwrap();
        if inner.state.phase != HarnessPhase::Idle {
            return Err(HarnessError::NotIdle(inner.state.phase));
        }
    }
    Ok(self
        .session
        .append(SessionEntryPayload::Custom { custom_type, data })
        .await?)
}
```

- [ ] **Step 4: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness/src/harness.rs
git commit -m "fix(harness): add Idle-only guard to append_message and append_custom_entry (P1-3)"
```

---

## Task 8: Validate `first_kept_entry` in `apply_compaction_result` (P1-4)

**Files:**
- Modify: `crates/llm-harness-types/src/errors.rs`
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Add `InvalidFirstKeptEntry` variant to `CompactionError`**

In `crates/llm-harness-types/src/errors.rs`, add to the `CompactionError` enum:

```rust
#[error("first_kept_entry {0} not found in compaction path")]
InvalidFirstKeptEntry(crate::EntryId),
```

- [ ] **Step 2: Write the failing test**

Add to the `tests` module in `harness.rs`:

```rust
#[tokio::test]
async fn compact_override_with_invalid_first_kept_entry_returns_error() {
    use llm_harness_types::{
        BeforeCompactCtx, BeforeCompactDecision, BeforeCompactHook, CompactionResult, EntryId,
    };
    use futures::future::BoxFuture;

    struct BadOverrideHook;

    impl BeforeCompactHook for BadOverrideHook {
        fn before_compact<'a>(
            &'a self,
            _ctx: BeforeCompactCtx<'a>,
        ) -> BoxFuture<'a, BeforeCompactDecision> {
            Box::pin(async move {
                // Return an Override with a random ID that does not exist in the path
                BeforeCompactDecision::Override(CompactionResult {
                    summary_message: AgentMessage::User(UserMessage {
                        content: vec![ContentBlock::Text { text: "summary".into() }],
                        timestamp: chrono::Utc::now(),
                    }),
                    first_kept_entry: EntryId::new(), // random / nonexistent
                    tokens_before: 100,
                    tokens_after: 10,
                    file_operations: vec![],
                })
            })
        }
    }

    let client = Arc::new(MockLlmClient::new(vec![MockResponse::text("hello")]));
    let env = Arc::new(NoOpEnv);
    let mut opts = AgentHarnessOptions::new("test-model");
    opts.hooks.before_compact = Some(Arc::new(BadOverrideHook));
    let h = futures::executor::block_on(AgentHarness::new_in_memory(client, env, opts));

    // Build up at least one message so the path is non-empty
    h.prompt("hello").await.unwrap();

    let err = h.compact().await;
    assert!(
        matches!(
            err,
            Err(HarnessError::Compaction(CompactionError::InvalidFirstKeptEntry(_)))
        ),
        "compact() must return InvalidFirstKeptEntry for a nonexistent entry: {err:?}"
    );
}
```

- [ ] **Step 3: Verify the test fails (Red)**

```bash
cargo test --package llm-harness --features test-utils compact_override_with_invalid
```
Expected: FAIL — current code does `unwrap_or(0)` and silently proceeds.

- [ ] **Step 4: Add validation in `apply_compaction_result`**

In `harness.rs`, at the start of `apply_compaction_result` (around line 1440), add before the `let tokens_before = ...` line:

```rust
if !path.iter().any(|e| e.id == result.first_kept_entry) {
    return Err(CompactionError::InvalidFirstKeptEntry(result.first_kept_entry).into());
}
```

- [ ] **Step 5: Run tests (Green)**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness-types/src/errors.rs crates/llm-harness/src/harness.rs
git commit -m "fix(harness): validate first_kept_entry in apply_compaction_result (P1-4)"
```

---

## Task 9: Fix `label_at` O(N) scan + `list_branches` O(L×M) path cloning (P1-5 + P1-6)

**Files:**
- Modify: `crates/llm-harness/src/session/storage.rs`
- Modify: `crates/llm-harness/src/session/jsonl.rs`
- Modify: `crates/llm-harness/src/session/session.rs`

Note: P1-6 (`label_at`) is the simpler fix and is done first. P1-5 (`list_branches`) adds a new trait method.

- [ ] **Step 1: Fix `label_at` in `InMemorySessionStorage` (storage.rs ~217)**

Replace the O(N) scan with a children-index lookup:

```rust
fn label_at(&self, id: EntryId) -> BoxFuture<'_, Result<Option<String>, SessionError>> {
    Box::pin(async move {
        let st = self.inner.lock().unwrap();
        let children = st.children.get(&Some(id)).map(|v| v.as_slice()).unwrap_or(&[]);
        for child_id in children {
            if let Some(e) = st.entries.get(child_id) {
                if let SessionEntryPayload::Label { name } = &e.payload {
                    return Ok(Some(name.clone()));
                }
            }
        }
        Ok(None)
    })
}
```

- [ ] **Step 2: Fix `label_at` in `JsonlSessionStorage` (jsonl.rs ~250)**

Apply the same children-index lookup (note: `JsonlSessionStorage` uses `tokio::sync::Mutex` — use `.await`):

```rust
fn label_at(&self, id: EntryId) -> BoxFuture<'_, Result<Option<String>, SessionError>> {
    Box::pin(async move {
        let mut inner = self.inner.lock().await;
        Self::ensure_loaded(&mut inner).await?;
        let children = inner
            .children_map
            .get(&Some(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        for child_id in children {
            if let Some(e) = inner.entry_map.get(child_id) {
                if let SessionEntryPayload::Label { name } = &e.payload {
                    return Ok(Some(name.clone()));
                }
            }
        }
        Ok(None)
    })
}
```

- [ ] **Step 3: Add `paths_to_all_leaves` to `SessionStorage` trait (storage.rs)**

Add the new required method to the `SessionStorage` trait:

```rust
/// Return all leaf-to-root paths in a single traversal.
///
/// Each inner `Vec` is ordered root-first (same convention as `path_to_root`).
/// Replaces L calls to `path_to_root` in `Session::list_branches`.
fn paths_to_all_leaves(&self) -> BoxFuture<'_, Result<Vec<Vec<SessionEntry>>, SessionError>>;
```

- [ ] **Step 4: Implement `paths_to_all_leaves` in `InMemorySessionStorage`**

Add after the `label_at` impl in `storage.rs`:

```rust
fn paths_to_all_leaves(&self) -> BoxFuture<'_, Result<Vec<Vec<SessionEntry>>, SessionError>> {
    Box::pin(async move {
        let st = self.inner.lock().unwrap();
        // Leaves: entry IDs that have no children in the index.
        let leaves: Vec<EntryId> = st
            .entries
            .keys()
            .filter(|id| {
                st.children
                    .get(&Some(**id))
                    .map(|v| v.is_empty())
                    .unwrap_or(true)
            })
            .copied()
            .collect();
        let mut result = Vec::with_capacity(leaves.len());
        for leaf_id in leaves {
            let mut path = Vec::new();
            let mut cur = Some(leaf_id);
            while let Some(id) = cur {
                if let Some(e) = st.entries.get(&id) {
                    path.push(e.clone());
                    cur = e.parent_id;
                } else {
                    break;
                }
            }
            path.reverse();
            result.push(path);
        }
        Ok(result)
    })
}
```

- [ ] **Step 5: Implement `paths_to_all_leaves` in `JsonlSessionStorage`**

Add after the `label_at` impl in `jsonl.rs`:

```rust
fn paths_to_all_leaves(&self) -> BoxFuture<'_, Result<Vec<Vec<SessionEntry>>, SessionError>> {
    Box::pin(async move {
        let mut inner = self.inner.lock().await;
        Self::ensure_loaded(&mut inner).await?;
        let leaves: Vec<EntryId> = inner
            .entry_map
            .keys()
            .filter(|id| {
                inner
                    .children_map
                    .get(&Some(**id))
                    .map(|v| v.is_empty())
                    .unwrap_or(true)
            })
            .copied()
            .collect();
        let mut result = Vec::with_capacity(leaves.len());
        for leaf_id in leaves {
            let mut path = Vec::new();
            let mut current = Some(leaf_id);
            while let Some(id) = current {
                match inner.entry_map.get(&id) {
                    Some(e) => {
                        path.push(e.clone());
                        current = e.parent_id;
                    }
                    None => return Err(SessionError::EntryNotFound(id)),
                }
            }
            path.reverse();
            result.push(path);
        }
        Ok(result)
    })
}
```

Also add `paths_to_all_leaves` to `FailAfterNStorage` (from Task 1):

```rust
fn paths_to_all_leaves(&self) -> BoxFuture<'_, Result<Vec<Vec<SessionEntry>>, SessionError>> {
    self.inner.paths_to_all_leaves()
}
```

- [ ] **Step 6: Update `Session::list_branches` to use `paths_to_all_leaves`**

Replace the implementation in `session.rs` (around line 181):

```rust
pub async fn list_branches(&self) -> Result<Vec<BranchInfo>, SessionError> {
    let all_paths = self.storage.paths_to_all_leaves().await?;
    let mut branches = Vec::with_capacity(all_paths.len());
    for path in all_paths {
        let Some(leaf_entry) = path.last() else { continue };
        let leaf_id = leaf_entry.id;
        let message_count = path.len();
        let last_activity = leaf_entry.timestamp;
        let label = self.storage.label_at(leaf_id).await?;
        let summary = path.iter().rev().find_map(|e| {
            if let SessionEntryPayload::BranchSummary(bs) = &e.payload
                && bs.leaf_id == leaf_id
            {
                Some(bs.summary.clone())
            } else {
                None
            }
        });
        branches.push(BranchInfo {
            leaf_id,
            label,
            message_count,
            last_activity,
            summary,
        });
    }
    Ok(branches)
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass. `list_branches` behavior is unchanged.

- [ ] **Step 8: Commit**

```bash
git add crates/llm-harness/src/session/storage.rs crates/llm-harness/src/session/jsonl.rs crates/llm-harness/src/session/session.rs crates/llm-harness/src/harness.rs
git commit -m "perf(session): fix label_at O(N) scan and list_branches O(L×M) path cloning (P1-5 + P1-6)"
```

---

## Task 10: Fix `estimated_tokens` rough calculation in `before_compact` hook (P2-1)

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Replace rough estimate with `estimate_tokens_for_entry` sum**

In `do_compact` (around line 1376–1378), replace:

```rust
// Old (rough):
let rough_tokens = path.len() * 100;
let decision = h
    .before_compact(BeforeCompactCtx {
        estimated_tokens: rough_tokens,
```

with:

```rust
// New (accurate):
let estimated_tokens: usize = path.iter().map(crate::compaction::estimate_tokens_for_entry).sum();
let decision = h
    .before_compact(BeforeCompactCtx {
        estimated_tokens,
```

Remove the `rough_tokens` variable.

- [ ] **Step 2: Verify compilation**

```bash
cargo check --package llm-harness --features test-utils
```
Expected: compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/llm-harness/src/harness.rs
git commit -m "fix(harness): use accurate token estimate for before_compact hook (P2-1)"
```

---

## Task 11: Fix `compressed_entries` silent fallback + replace `unwrap_or(0)` with `expect` (P2-2)

**Prerequisite:** Task 8 must be complete (P1-4 validation is in place).

**Files:**
- Modify: `crates/llm-harness/src/harness.rs`

- [ ] **Step 1: Replace `unwrap_or(0)` with `expect` in `apply_compaction_result`**

In `apply_compaction_result` (around lines 1447–1450), replace:

```rust
// Old:
let compressed_entries = path
    .iter()
    .position(|e| e.id == result.first_kept_entry)
    .unwrap_or(0);
```

with:

```rust
// New: Task 8 validated first_kept_entry is in path, so unwrap is safe.
let compressed_entries = path
    .iter()
    .position(|e| e.id == result.first_kept_entry)
    .expect("first_kept_entry validated before apply_compaction_result");
```

- [ ] **Step 2: Verify compilation and tests**

```bash
cargo test --package llm-harness --features test-utils
```
Expected: all tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/llm-harness/src/harness.rs
git commit -m "fix(harness): replace compressed_entries unwrap_or(0) with expect after validation (P2-2)"
```

---

## Final Verification

After all tasks:

- [ ] **Run `cargo fmt`**

```bash
cargo fmt --all
```

- [ ] **Run `cargo clippy`**

```bash
cargo clippy --all-targets --all-features
```
Expected: zero warnings. Fix any clippy warnings before declaring done.

- [ ] **Run full test suite**

```bash
cargo test --all --features test-utils
```
Expected: all tests pass.

---

## Self-Review Notes

**Spec coverage gaps checked:**
- P0-1 (run_loop cleanup): covered by Tasks 1
- P0-2 (ToolCallEnd tool_name): covered by Task 2
- P0-3~P0-13 (doc comments): covered by Tasks 3 and 4
- P1-1 (clear queues): covered by Task 5
- P1-2 (replace transcript): covered by Task 6
- P1-3 (phase check append): covered by Task 7
- P1-4 (validate first_kept_entry): covered by Task 8
- P1-5 (list_branches perf): covered by Task 9
- P1-6 (label_at O(N)): covered by Task 9
- P2-1 (estimated_tokens): covered by Task 10
- P2-2 (compressed_entries): covered by Task 11

**Follow-up notes from review (not in scope):**
- `ForkOptions::copy_entries` doc clarification: cosmetic only, skip.
- `AgentHarnessOptions::new()` dummy channel cleanup: separate cleanup task.
- `generate_branch_summary` concurrency doc: add doc note to method if desired.
- `skills()`/`templates()` clone cost: not in P0/P1/P2.
