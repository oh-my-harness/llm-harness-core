# Phase 1: llm-harness-types Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `llm-harness-types` crate — the zero-IO pure-types layer shared across all harness crates.

**Architecture:** Cargo workspace with three crates; this phase creates the workspace skeleton and implements `crates/llm-harness-types` with all identifiers, error types, message types, event types, Tool/ExecutionEnv traits, hook traits, and miscellaneous types. No IO; depends only on `serde`, `serde_json`, `futures`, `tokio` (sync), `tokio-util`, `thiserror`, `uuid`, `chrono`, `anyhow`.

**Tech Stack:** Rust 2024 edition, `thiserror 2`, `serde 1`, `serde_json 1`, `uuid 1` (v7), `chrono 0.4`, `tokio 1`, `tokio-util 0.7`, `futures 0.3`, `anyhow 1`

---

## File Map

| File | Responsibility |
|---|---|
| `Cargo.toml` (workspace) | workspace members declaration |
| `crates/llm-harness-types/Cargo.toml` | crate metadata + dependencies |
| `crates/llm-harness-types/src/lib.rs` | module declarations + pub re-exports |
| `crates/llm-harness-types/src/identity.rs` | `EntryId` |
| `crates/llm-harness-types/src/errors.rs` | all error enums + `DiagnosticLevel` + `HarnessPhase` |
| `crates/llm-harness-types/src/content.rs` | `ContentBlock`, `ImageSource` |
| `crates/llm-harness-types/src/messages.rs` | `AgentMessage` + all message structs + `TokenUsage` |
| `crates/llm-harness-types/src/events.rs` | `AgentEvent` |
| `crates/llm-harness-types/src/tool.rs` | `Tool` trait, `ToolContext`, `ToolResult`, `ToolExecutionMode` |
| `crates/llm-harness-types/src/env.rs` | `ExecutionEnv` trait, `ShellOptions`, `ShellOutput`, `FileInfo` |
| `crates/llm-harness-types/src/hooks.rs` | all hook traits and their context/decision types |
| `crates/llm-harness-types/src/misc.rs` | `ThinkingLevel`, `AgentContext`, `TurnSnapshot`, `StreamOptions` |
| `crates/llm-harness-types/src/resources.rs` | `AgentHarnessResources` stub (referenced by `BeforeRunCtx`) |

---

### Task 1: Workspace and crate skeleton

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/llm-harness-types/Cargo.toml`
- Create: `crates/llm-harness-types/src/lib.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
[workspace]
members = ["crates/llm-harness-types"]
resolver = "2"

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT"

[workspace.dependencies]
anyhow     = "1"
chrono     = { version = "0.4", features = ["serde"] }
futures    = "0.3"
serde      = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror  = "2"
tokio      = { version = "1", features = ["sync"] }
tokio-util = { version = "0.7", features = ["rt"] }
uuid       = { version = "1", features = ["v7", "serde"] }
```

Save to: `/Users/hhl/Documents/projs/llm-harness-core/Cargo.toml`

- [ ] **Step 2: Create crates/llm-harness-types/Cargo.toml**

```toml
[package]
name    = "llm-harness-types"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace     = true
chrono.workspace     = true
futures.workspace    = true
serde.workspace      = true
serde_json.workspace = true
thiserror.workspace  = true
tokio.workspace      = true
tokio-util.workspace = true
uuid.workspace       = true
```

Save to: `crates/llm-harness-types/Cargo.toml`

- [ ] **Step 3: Create lib.rs with module declarations**

```rust
pub mod content;
pub mod env;
pub mod errors;
pub mod events;
pub mod hooks;
pub mod identity;
pub mod messages;
pub mod misc;
pub mod resources;
pub mod tool;

pub use content::*;
pub use env::*;
pub use errors::*;
pub use events::*;
pub use hooks::*;
pub use identity::*;
pub use messages::*;
pub use misc::*;
pub use resources::*;
pub use tool::*;
```

Save to: `crates/llm-harness-types/src/lib.rs`

- [ ] **Step 4: Verify it compiles (empty modules)**

Create placeholder `mod.rs` stubs for each module (empty files) then run:

```bash
cd /Users/hhl/Documents/projs/llm-harness-core
cargo check -p llm-harness-types
```

Expected: PASS (empty modules compile fine)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/
git commit -m "chore(types): bootstrap workspace and llm-harness-types crate skeleton"
```

---

### Task 2: EntryId

**Files:**
- Create: `crates/llm-harness-types/src/identity.rs`

- [ ] **Step 1: Write the failing test**

In `crates/llm-harness-types/src/identity.rs` add a `#[cfg(test)]` block:

```rust
use crate::EntryId;
use std::str::FromStr;

#[test]
fn entry_id_roundtrip_display_fromstr() {
    let id = EntryId::new();
    let s = id.to_string();
    let id2 = EntryId::from_str(&s).unwrap();
    assert_eq!(id, id2);
}

#[test]
fn entry_id_serde_roundtrip() {
    let id = EntryId::new();
    let json = serde_json::to_string(&id).unwrap();
    // serialized as a bare string (no object wrapper)
    assert!(json.starts_with('"'));
    let id2: EntryId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, id2);
}

#[test]
fn entry_id_is_time_ordered() {
    // UUIDv7 guarantees monotone ordering within the same ms bucket
    let a = EntryId::new();
    // tiny sleep to guarantee different ms timestamp
    std::thread::sleep(std::time::Duration::from_millis(2));
    let b = EntryId::new();
    assert!(a.0 < b.0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types entry_id 2>&1 | head -20
```

Expected: FAIL — `EntryId` not yet defined.

- [ ] **Step 3: Implement EntryId**

```rust
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    str::FromStr,
};
use uuid::Uuid;

/// Session log 中每条 entry 的唯一标识，UUIDv7（时间有序）。
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct EntryId(pub Uuid);

impl EntryId {
    /// Generate a new time-ordered UUIDv7 identifier.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for EntryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for EntryId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Serialize for EntryId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EntryId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_id_roundtrip_display_fromstr() {
        let id = EntryId::new();
        let s = id.to_string();
        let id2 = EntryId::from_str(&s).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn entry_id_serde_roundtrip() {
        let id = EntryId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"'));
        let id2: EntryId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn entry_id_is_time_ordered() {
        let a = EntryId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = EntryId::new();
        assert!(a.0 < b.0);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types entry_id
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/identity.rs
git commit -m "feat(types): implement EntryId (UUIDv7 newtype)"
```

---

### Task 3: Error types and HarnessPhase

**Files:**
- Create: `crates/llm-harness-types/src/errors.rs`

- [ ] **Step 1: Write failing tests**

Add to `errors.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_error_display() {
        let e = ToolError::InvalidArguments("bad arg".into());
        assert!(e.to_string().contains("bad arg"));
        let e2 = ToolError::Aborted;
        assert_eq!(e2.to_string(), "tool aborted");
    }

    #[test]
    fn agent_error_is_clone() {
        let e = AgentError::Provider("rate limit".into());
        let e2 = e.clone();
        assert_eq!(e.to_string(), e2.to_string());
    }

    #[test]
    fn harness_error_from_agent_error() {
        let ae = AgentError::Aborted;
        let he: HarnessError = ae.into();
        assert!(he.to_string().contains("aborted"));
    }

    #[test]
    fn compaction_error_from_session_error() {
        use crate::EntryId;
        let se = SessionError::EntryNotFound(EntryId::new());
        let ce: CompactionError = se.into();
        assert!(ce.to_string().contains("entry not found"));
    }

    #[test]
    fn stop_reason_copy() {
        let r = StopReason::ToolUse;
        let r2 = r; // copy
        assert_eq!(r, r2);
    }

    #[test]
    fn harness_phase_not_idle_in_error() {
        let e = HarnessError::NotIdle(HarnessPhase::Turning);
        assert!(e.to_string().contains("Turning"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types errors 2>&1 | head -20
```

Expected: FAIL — types not yet defined.

- [ ] **Step 3: Implement all error types**

```rust
use std::path::PathBuf;

// ── StopReason ────────────────────────────────────────────────────────────────

/// LLM 停止生成的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// 模型自然结束。
    EndTurn,
    /// 达到 max_tokens 限制被截断。
    MaxTokens,
    /// 匹配到停止序列。
    StopSequence,
    /// 模型请求调用工具。
    ToolUse,
    /// 未知或未分类的停止原因。
    Other,
}

// ── DiagnosticLevel ───────────────────────────────────────────────────────────

/// Skills / PromptTemplates 加载过程中产生的诊断级别。
#[derive(Debug, Clone, Copy)]
pub enum DiagnosticLevel {
    /// 可继续的警告（如单个文件格式非法）。
    Warn,
    /// 整体操作失败。
    Error,
}

// ── HarnessPhase ──────────────────────────────────────────────────────────────

/// AgentHarness 的运行阶段。
///
/// 提升到 types crate 是因为 `HarnessError::NotIdle` 需要携带它，
/// 而 `HarnessError` 在 types 中定义。
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum HarnessPhase {
    /// 无正在进行的操作，接受新请求。
    Idle,
    /// 正在执行 agent loop（LLM 调用 + tool 执行）。
    Turning,
    /// 正在执行 compaction。
    Compacting,
    /// 正在执行分支导航。
    Branching,
}

// ── ToolError ─────────────────────────────────────────────────────────────────

/// Tool 执行失败。
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool aborted")]
    Aborted,
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// ── AgentError ────────────────────────────────────────────────────────────────

/// Agent 运行时错误。
#[derive(thiserror::Error, Debug, Clone)]
pub enum AgentError {
    #[error("llm provider error: {0}")]
    Provider(String),
    #[error("tool error: {tool_name}: {message}")]
    Tool { tool_name: String, message: String },
    #[error("aborted")]
    Aborted,
    #[error("agent is not idle")]
    NotIdle,
    #[error("internal: {0}")]
    Internal(String),
}

// ── EnvError ──────────────────────────────────────────────────────────────────

/// 执行环境错误。
#[derive(thiserror::Error, Debug)]
pub enum EnvError {
    #[error("path not found: {0}")]
    NotFound(PathBuf),
    #[error("permission denied: {0}")]
    PermissionDenied(PathBuf),
    #[error("path already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("is a directory: {0}")]
    IsADirectory(PathBuf),
    #[error("operation aborted")]
    Aborted,
    #[error("invalid utf-8 in {0}")]
    InvalidUtf8(PathBuf),
    #[error("shell command failed: exit {exit_code}")]
    ShellFailed { exit_code: i32, stderr: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("other: {0}")]
    Other(String),
}

// ── SessionError ──────────────────────────────────────────────────────────────

/// Session 操作错误。
#[derive(thiserror::Error, Debug)]
pub enum SessionError {
    #[error("entry not found: {0}")]
    EntryNotFound(crate::EntryId),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("session already exists: {0}")]
    SessionAlreadyExists(String),
    #[error("not a leaf: {0}")]
    NotALeaf(crate::EntryId),
    #[error("invalid parent: {0}")]
    InvalidParent(crate::EntryId),
    #[error("storage io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Serialization(String),
    #[error("concurrent modification")]
    ConcurrentModification,
}

// ── CompactionError ───────────────────────────────────────────────────────────

/// Compaction 操作错误。
#[derive(thiserror::Error, Debug)]
pub enum CompactionError {
    #[error("not enough tokens to compact")]
    InsufficientTokens,
    #[error("summary model call failed: {0}")]
    SummaryFailed(String),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Agent(#[from] AgentError),
}

// ── TemplateError ─────────────────────────────────────────────────────────────

/// PromptTemplate 错误。
#[derive(thiserror::Error, Debug)]
pub enum TemplateError {
    #[error("template not found: {0}")]
    NotFound(String),
    #[error("missing required argument at position {0}")]
    MissingArg(usize),
    #[error("invalid argument syntax: {0}")]
    InvalidSyntax(String),
}

// ── HarnessError ──────────────────────────────────────────────────────────────

/// AgentHarness 顶层错误。
#[derive(thiserror::Error, Debug)]
pub enum HarnessError {
    #[error("harness is not idle (current phase: {0:?})")]
    NotIdle(HarnessPhase),
    #[error("skill not found: {0}")]
    SkillNotFound(String),
    #[error("template not found: {0}")]
    TemplateNotFound(String),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Compaction(#[from] CompactionError),
    #[error(transparent)]
    Env(#[from] EnvError),
    #[error(transparent)]
    Template(#[from] TemplateError),
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types errors
```

Expected: 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/errors.rs
git commit -m "feat(types): implement error types, StopReason, DiagnosticLevel, HarnessPhase"
```

---

### Task 4: ContentBlock and ImageSource

**Files:**
- Create: `crates/llm-harness-types/src/content.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_serde_text() {
        let cb = ContentBlock::Text { text: "hello".into() };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(matches!(cb2, ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn content_block_serde_tool_use() {
        let cb = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(matches!(cb2, ContentBlock::ToolUse { name, .. } if name == "read_file"));
    }

    #[test]
    fn content_block_serde_thinking() {
        let cb = ContentBlock::Thinking {
            thinking: "let me think".into(),
            signature: Some("sig123".into()),
        };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(cb2, ContentBlock::Thinking { signature: Some(s), .. } if s == "sig123")
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types content 2>&1 | head -20
```

Expected: FAIL — `ContentBlock` not yet defined.

- [ ] **Step 3: Implement content.rs**

```rust
use serde::{Deserialize, Serialize};

/// 图片数据来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64 编码的内联图片。
    Base64 {
        /// MIME 类型，如 `"image/png"`。
        media_type: String,
        /// Base64 编码的图片数据。
        data: String,
    },
}

/// 消息内容的最小单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// 普通文本。
    Text {
        /// 文本内容。
        text: String,
    },
    /// Provider 推理/思考内容（Anthropic、OpenAI o 系列、DeepSeek R 系列等）。
    Thinking {
        /// 思考内容。
        thinking: String,
        /// Anthropic 特有的内容签名；其他 provider 置 `None`。
        signature: Option<String>,
    },
    /// 图片内容。
    Image {
        /// 图片数据来源。
        source: ImageSource,
    },
    /// LLM 发出的工具调用请求。
    ToolUse {
        /// LLM 分配的工具调用唯一 ID。
        id: String,
        /// 工具名称。
        name: String,
        /// 工具调用参数（JSON）。
        input: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_block_serde_text() {
        let cb = ContentBlock::Text { text: "hello".into() };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(matches!(cb2, ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn content_block_serde_tool_use() {
        let cb = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(matches!(cb2, ContentBlock::ToolUse { name, .. } if name == "read_file"));
    }

    #[test]
    fn content_block_serde_thinking() {
        let cb = ContentBlock::Thinking {
            thinking: "let me think".into(),
            signature: Some("sig123".into()),
        };
        let json = serde_json::to_string(&cb).unwrap();
        let cb2: ContentBlock = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(cb2, ContentBlock::Thinking { signature: Some(s), .. } if s == "sig123")
        );
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types content
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/content.rs
git commit -m "feat(types): implement ContentBlock and ImageSource"
```

---

### Task 5: Message types

**Files:**
- Create: `crates/llm-harness-types/src/messages.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentBlock;

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text { text: s.into() }
    }

    #[test]
    fn agent_message_serde_user() {
        let msg = AgentMessage::User(UserMessage {
            content: vec![text_block("hello")],
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::User(_)));
    }

    #[test]
    fn agent_message_serde_assistant() {
        let msg = AgentMessage::Assistant(AssistantMessage {
            content: vec![text_block("response")],
            stop_reason: Some(crate::StopReason::EndTurn),
            timestamp: chrono::Utc::now(),
            provider: Some("anthropic".into()),
            api: Some("messages".into()),
            model: Some("claude-3".into()),
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
            error_message: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::Assistant(_)));
    }

    #[test]
    fn tool_result_message_serde() {
        let msg = AgentMessage::ToolResult(ToolResultMessage {
            tool_use_id: "call_1".into(),
            content: vec![text_block("result")],
            is_error: false,
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::ToolResult(_)));
    }

    #[test]
    fn custom_message_serde() {
        let msg = AgentMessage::Custom(CustomMessage {
            r#type: "artifact".into(),
            data: serde_json::json!({"url": "https://example.com"}),
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::Custom(_)));
    }

    #[test]
    fn token_usage_total() {
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 20,
            cache_creation_tokens: 10,
        };
        assert_eq!(u.total_tokens(), 180);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types messages 2>&1 | head -20
```

Expected: FAIL — types not defined yet.

- [ ] **Step 3: Implement messages.rs**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ContentBlock, StopReason};

/// 单次 LLM 调用的 token 用量。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    /// Prompt token 数。
    pub input_tokens: u32,
    /// Completion token 数。
    pub output_tokens: u32,
    /// Anthropic prompt cache 命中的 token 数。
    pub cache_read_tokens: u32,
    /// Anthropic prompt cache 新写入的 token 数。
    pub cache_creation_tokens: u32,
}

impl TokenUsage {
    /// 所有 token 的合计。
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_creation_tokens
    }
}

/// 用户消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    /// 消息内容块列表。
    pub content: Vec<ContentBlock>,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
}

/// LLM 助手消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// 消息内容块列表（含 Text、Thinking、ToolUse 等）。
    pub content: Vec<ContentBlock>,
    /// LLM 停止生成的原因。
    pub stop_reason: Option<StopReason>,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
    /// 生成此消息的 LLM provider 名（如 `"anthropic"`）。
    pub provider: Option<String>,
    /// 使用的 API 类型（如 `"messages"`、`"chat"`）。
    pub api: Option<String>,
    /// 使用的模型 ID。
    pub model: Option<String>,
    /// 本次调用的 token 用量；compaction 估算依赖此字段。
    pub usage: Option<TokenUsage>,
    /// LLM 返回错误时保存的错误文本快照。
    pub error_message: Option<String>,
}

/// 工具执行结果消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    /// 对应的 LLM tool call ID。
    pub tool_use_id: String,
    /// 发送给 LLM 的结果内容块。
    pub content: Vec<ContentBlock>,
    /// 工具执行是否失败。
    pub is_error: bool,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
}

/// Compaction 生成的摘要消息；由框架在 compaction 时插入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSummaryMessage {
    /// 摘要文本。
    pub summary: String,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
}

/// 分支导航时生成的摘要消息；由框架在 navigate_tree 时插入。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryMessage {
    /// 摘要文本。
    pub summary: String,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
}

/// 应用层自定义消息；必须由 `ConvertToLlmHook` 提供转换器才能进入 LLM 上下文。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMessage {
    /// 应用层自定义的消息类别标签（如 `"artifact"`、`"notification"`）。
    pub r#type: String,
    /// 任意 JSON 负载。
    pub data: serde_json::Value,
    /// 消息创建时间。
    pub timestamp: DateTime<Utc>,
}

/// Agent 内部消息联合体；会进入 session log 并经 compaction 处理。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum AgentMessage {
    /// 用户消息。
    User(UserMessage),
    /// LLM 助手消息。
    Assistant(AssistantMessage),
    /// 工具执行结果消息。
    ToolResult(ToolResultMessage),
    /// 框架生成的分支摘要消息。
    BranchSummary(BranchSummaryMessage),
    /// 框架生成的 compaction 摘要消息。
    CompactionSummary(CompactionSummaryMessage),
    /// 应用层自定义消息。
    Custom(CustomMessage),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentBlock;

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text { text: s.into() }
    }

    #[test]
    fn agent_message_serde_user() {
        let msg = AgentMessage::User(UserMessage {
            content: vec![text_block("hello")],
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::User(_)));
    }

    #[test]
    fn agent_message_serde_assistant() {
        let msg = AgentMessage::Assistant(AssistantMessage {
            content: vec![text_block("response")],
            stop_reason: Some(crate::StopReason::EndTurn),
            timestamp: chrono::Utc::now(),
            provider: Some("anthropic".into()),
            api: Some("messages".into()),
            model: Some("claude-3".into()),
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            }),
            error_message: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::Assistant(_)));
    }

    #[test]
    fn tool_result_message_serde() {
        let msg = AgentMessage::ToolResult(ToolResultMessage {
            tool_use_id: "call_1".into(),
            content: vec![text_block("result")],
            is_error: false,
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::ToolResult(_)));
    }

    #[test]
    fn custom_message_serde() {
        let msg = AgentMessage::Custom(CustomMessage {
            r#type: "artifact".into(),
            data: serde_json::json!({"url": "https://example.com"}),
            timestamp: chrono::Utc::now(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let msg2: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(msg2, AgentMessage::Custom(_)));
    }

    #[test]
    fn token_usage_total() {
        let u = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 20,
            cache_creation_tokens: 10,
        };
        assert_eq!(u.total_tokens(), 180);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types messages
```

Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/messages.rs
git commit -m "feat(types): implement AgentMessage and all message structs"
```

---

### Task 6: AgentEvent

**Files:**
- Create: `crates/llm-harness-types/src/events.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentError, AgentMessage, AssistantMessage, ContentBlock, StopReason, TokenUsage, ToolError, ToolResult};

    fn make_assistant() -> AssistantMessage {
        AssistantMessage {
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop_reason: Some(StopReason::EndTurn),
            timestamp: chrono::Utc::now(),
            provider: None,
            api: None,
            model: None,
            usage: None,
            error_message: None,
        }
    }

    #[test]
    fn agent_start_event_has_messages() {
        let ev = AgentEvent::AgentStart { initial_messages: vec![] };
        assert!(matches!(ev, AgentEvent::AgentStart { .. }));
    }

    #[test]
    fn agent_end_event_carries_new_messages() {
        let ev = AgentEvent::AgentEnd { new_messages: vec![] };
        assert!(matches!(ev, AgentEvent::AgentEnd { .. }));
    }

    #[test]
    fn turn_end_carries_tool_results() {
        let result: Result<ToolResult, ToolError> = Ok(ToolResult {
            content: vec![],
            details: serde_json::Value::Null,
            terminate: false,
        });
        let ev = AgentEvent::TurnEnd {
            index: 0,
            message: make_assistant(),
            tool_results: vec![("call_1".into(), result)],
        };
        assert!(matches!(ev, AgentEvent::TurnEnd { index: 0, .. }));
    }

    #[test]
    fn error_event() {
        let ev = AgentEvent::Error(AgentError::Aborted);
        assert!(matches!(ev, AgentEvent::Error(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types events 2>&1 | head -20
```

Expected: FAIL — `AgentEvent` not yet defined (also `ToolResult` not defined yet; that's fine, it'll fail on missing types).

- [ ] **Step 3: Implement events.rs**

Note: `ToolResult` is defined in `tool.rs` (Task 7). Add a forward reference comment, then implement both files in the same task order. For now write the full events.rs; it won't compile until `tool.rs` is also written (Task 7 runs immediately after).

```rust
use std::sync::Arc;

use crate::{AgentError, AgentMessage, AssistantMessage, ToolError, ToolResult};

/// Agent 产生的完整事件流。
///
/// 消息级事件（`Message*`）与 token 级事件（`TextDelta` 等）并列，
/// 支持消息列表 UI 和字符流 UI 两种模式。
#[derive(Debug)]
pub enum AgentEvent {
    // === Agent 生命周期 ===
    /// Agent 开始一次完整运行；携带本次注入的初始消息。
    AgentStart { initial_messages: Vec<AgentMessage> },
    /// Agent 完成本次运行；携带本次新增的全部消息（Harness 的关键接口契约）。
    AgentEnd { new_messages: Vec<AgentMessage> },

    // === Turn 生命周期 ===
    /// 一次 turn 开始；`index` 从 0 开始递增。
    TurnStart { index: u32 },
    /// 一次 turn 结束；携带完整 assistant message 和全部 tool 执行结果。
    TurnEnd {
        /// Turn 编号（从 0 开始）。
        index: u32,
        /// 本轮 LLM 回复。
        message: AssistantMessage,
        /// 本轮所有 tool 执行结果；key 为 `tool_use_id`。
        tool_results: Vec<(String, Result<ToolResult, ToolError>)>,
    },

    // === 消息级（assistant message 边界） ===
    /// 一条新的 assistant message 开始流式生成。
    MessageStart { message_id: String },
    /// 流式期间 assistant message 的当前快照（覆盖之前的快照）。
    MessageUpdate { message_id: String, partial: AssistantMessage },
    /// Assistant message 完整生成完毕，含 stop_reason 和 usage。
    MessageEnd { message_id: String, message: AssistantMessage },

    // === Token 级（字符流） ===
    /// 文本增量。
    TextDelta { message_id: String, text: String },
    /// 推理/思考内容增量。
    ThinkingDelta { message_id: String, thinking: String, signature: Option<String> },
    /// LLM 开始请求调用某个工具。
    ToolCallStart { message_id: String, tool_use_id: String, name: String },
    /// LLM 工具调用参数的增量 JSON 片段。
    ToolCallArgsDelta { tool_use_id: String, partial_input: String },
    /// LLM 工具调用参数完整到达，含解析后的完整参数。
    ToolCallEnd { tool_use_id: String, args: serde_json::Value },

    // === 工具执行（Rust 层面执行 tool，区别于 LLM 发起的 ToolCall） ===
    /// Tool 开始执行。
    ToolExecutionStart { tool_use_id: String, tool_name: String, args: serde_json::Value },
    /// 长时间运行的 tool 推送的中间结果。
    ToolExecutionUpdate { tool_use_id: String, partial: ToolResult },
    /// Tool 执行完毕；携带 Rust 层面的执行结果。
    ToolExecutionEnd { tool_use_id: String, result: Result<ToolResult, ToolError> },

    /// Loop 遇到不可恢复的错误；之后 `AgentEnd` 将立即到达。
    Error(AgentError),
}
```

- [ ] **Step 4: Implement tool.rs (Task 7, required before events compiles)**

Implement `crates/llm-harness-types/src/tool.rs` (see Task 7 below) in the same edit session, then run:

```bash
cargo check -p llm-harness-types
```

Expected: PASS.

- [ ] **Step 5: Run event tests**

```bash
cargo test -p llm-harness-types events
```

Expected: 4 tests pass.

---

### Task 7: Tool trait, ToolContext, ToolResult

**Files:**
- Create: `crates/llm-harness-types/src/tool.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentBlock;

    #[test]
    fn tool_result_defaults_terminate_false() {
        let r = ToolResult {
            content: vec![ContentBlock::Text { text: "done".into() }],
            details: serde_json::Value::Null,
            terminate: false,
        };
        assert!(!r.terminate);
    }

    #[test]
    fn tool_execution_mode_default_is_parallel() {
        // A concrete impl that doesn't override execution_mode
        struct MyTool;
        impl Tool for MyTool {
            fn name(&self) -> &str { "my_tool" }
            fn description(&self) -> &str { "does stuff" }
            fn parameters_schema(&self) -> &serde_json::Value {
                &serde_json::Value::Null
            }
            fn execute<'a>(
                &'a self,
                _args: serde_json::Value,
                _ctx: &'a ToolContext,
            ) -> futures::future::BoxFuture<'a, Result<ToolResult, crate::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        content: vec![],
                        details: serde_json::Value::Null,
                        terminate: false,
                    })
                })
            }
        }
        let t = MyTool;
        assert!(matches!(t.execution_mode(), ToolExecutionMode::Parallel));
        assert_eq!(t.label(), "my_tool"); // default label = name
    }

    #[test]
    fn tool_is_object_safe() {
        fn _accepts(_: &dyn Tool) {}
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types tool 2>&1 | head -20
```

Expected: FAIL — types not defined.

- [ ] **Step 3: Implement tool.rs**

```rust
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AssistantMessage, ContentBlock, EnvError, ExecutionEnv, ToolError};

/// 工具执行结果。
#[derive(Debug, Clone)]
pub struct ToolResult {
    /// 发送给 LLM 的内容块列表。
    pub content: Vec<ContentBlock>,
    /// 不发送给 LLM 的结构化数据，用于 UI 渲染或审计日志。
    pub details: serde_json::Value,
    /// 当 batch 中所有 tool 均返回 `true` 时，agent loop 提前停止。
    pub terminate: bool,
}

/// 工具执行模式——决定 tool 在同一 batch 中的并发策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    /// 与同 batch 中的其他 `Parallel` tool 并发执行（`join_all`）。
    Parallel,
    /// 作为子组分割点——前一子组结束后才开始新子组。
    Sequential,
}

/// Tool 执行上下文，每次调用时由 loop 构造后传入。
pub struct ToolContext {
    /// 执行环境（文件系统 + shell）。
    pub env: Arc<dyn ExecutionEnv>,
    /// 用户取消信号。
    pub abort: CancellationToken,
    /// 当前 tool call 在 LLM 输出中的唯一 ID。
    pub tool_use_id: String,
    /// 当前轮次索引（从 0 开始）。
    pub turn_index: u32,
    /// 触发本次 tool call 的完整 LLM 响应；同一消息的多个 tool call 共享同一 Arc。
    pub assistant_message: Arc<AssistantMessage>,
    /// 长时间运行的 tool 通过此 channel 推送部分结果；接收端转发为 `AgentEvent::ToolExecutionUpdate`。
    pub update_tx: mpsc::Sender<ToolResult>,
}

/// 工具 trait——框架调用工具的唯一接口。
pub trait Tool: Send + Sync {
    /// 工具的稳定程序标识符；在 session log 和 LLM tool definition 中使用。
    fn name(&self) -> &str;

    /// 工具的人类可读 UI 显示名；默认回退到 `name()`。
    fn label(&self) -> &str {
        self.name()
    }

    /// 工具功能的自然语言描述，用于 LLM tool definition。
    fn description(&self) -> &str;

    /// 工具参数的 JSON Schema。
    fn parameters_schema(&self) -> &serde_json::Value;

    /// 工具在同一 batch 中的执行模式；默认 `Parallel`。
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// 在 schema 校验前对 LLM 原始参数做兼容转换；默认 identity（不转换）。
    fn prepare_arguments(
        &self,
        raw: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        Ok(raw)
    }

    /// 执行工具；返回结果或错误。
    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ContentBlock;

    #[test]
    fn tool_result_defaults_terminate_false() {
        let r = ToolResult {
            content: vec![ContentBlock::Text { text: "done".into() }],
            details: serde_json::Value::Null,
            terminate: false,
        };
        assert!(!r.terminate);
    }

    #[test]
    fn tool_execution_mode_default_is_parallel() {
        struct MyTool;
        impl Tool for MyTool {
            fn name(&self) -> &str { "my_tool" }
            fn description(&self) -> &str { "does stuff" }
            fn parameters_schema(&self) -> &serde_json::Value {
                &serde_json::Value::Null
            }
            fn execute<'a>(
                &'a self,
                _args: serde_json::Value,
                _ctx: &'a ToolContext,
            ) -> BoxFuture<'a, Result<ToolResult, crate::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        content: vec![],
                        details: serde_json::Value::Null,
                        terminate: false,
                    })
                })
            }
        }
        let t = MyTool;
        assert!(matches!(t.execution_mode(), ToolExecutionMode::Parallel));
        assert_eq!(t.label(), "my_tool");
    }

    #[test]
    fn tool_is_object_safe() {
        fn _accepts(_: &dyn Tool) {}
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types tool
cargo test -p llm-harness-types events
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/tool.rs crates/llm-harness-types/src/events.rs
git commit -m "feat(types): implement Tool trait, ToolContext, ToolResult, AgentEvent"
```

---

### Task 8: ExecutionEnv trait

**Files:**
- Create: `crates/llm-harness-types/src/env.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_output_fields() {
        let o = ShellOutput {
            stdout: "hello".into(),
            stderr: "".into(),
            exit_code: 0,
        };
        assert_eq!(o.exit_code, 0);
    }

    #[test]
    fn file_info_is_dir() {
        let fi = FileInfo {
            path: std::path::PathBuf::from("/tmp"),
            is_dir: true,
            size: 0,
            modified: chrono::Utc::now(),
        };
        assert!(fi.is_dir);
    }

    #[test]
    fn execution_env_is_object_safe() {
        fn _accepts(_: &dyn ExecutionEnv) {}
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types env 2>&1 | head -20
```

Expected: FAIL — `ExecutionEnv` not defined.

- [ ] **Step 3: Implement env.rs**

```rust
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::EnvError;

/// Shell 命令执行输出。
pub struct ShellOutput {
    /// 标准输出内容。
    pub stdout: String,
    /// 标准错误内容。
    pub stderr: String,
    /// 进程退出码。
    pub exit_code: i32,
}

/// 文件或目录的元数据。
pub struct FileInfo {
    /// 条目的绝对路径。
    pub path: PathBuf,
    /// 是否为目录。
    pub is_dir: bool,
    /// 文件大小（字节）；目录为 0。
    pub size: u64,
    /// 最后修改时间。
    pub modified: DateTime<Utc>,
}

/// Shell 命令执行选项。
pub struct ShellOptions<'a> {
    /// 覆盖工作目录；`None` 表示使用 env 默认工作目录。
    pub cwd: Option<&'a Path>,
    /// 额外注入的环境变量。
    pub env: Vec<(&'a str, &'a str)>,
    /// 超时时长；`None` 表示无超时。
    pub timeout: Option<Duration>,
    /// 取消信号。
    pub abort: CancellationToken,
    /// 流式 stdout 回调；`None` 时仅在最终 Output 中返回完整内容。
    pub on_stdout: Option<Box<dyn FnMut(&str) + Send + 'a>>,
    /// 流式 stderr 回调；`None` 时仅在最终 Output 中返回完整内容。
    pub on_stderr: Option<Box<dyn FnMut(&str) + Send + 'a>>,
}

/// 执行环境抽象——将文件系统和 shell 操作与具体平台解耦。
///
/// 实现方可以是本地 OS、Docker 容器、WASM 沙箱或测试 mock。
pub trait ExecutionEnv: Send + Sync {
    /// 返回 env 的默认工作目录。
    fn working_dir(&self) -> &Path;

    /// 读取文本文件；非 UTF-8 内容返回 `EnvError::InvalidUtf8`。
    fn read_text_file<'a>(
        &'a self,
        path: &'a Path,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<String, EnvError>>;

    /// 读取文本文件的行；`max_lines` 为 `None` 时读取全部行。
    fn read_text_lines<'a>(
        &'a self,
        path: &'a Path,
        max_lines: Option<usize>,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<String>, EnvError>>;

    /// 读取二进制文件的原始字节。
    fn read_binary_file<'a>(
        &'a self,
        path: &'a Path,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<u8>, EnvError>>;

    /// 写入文件（覆盖）。
    fn write_file<'a>(
        &'a self,
        path: &'a Path,
        content: &'a [u8],
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<(), EnvError>>;

    /// 追加内容到文件末尾（JSONL 存储的核心操作）。
    fn append_file<'a>(
        &'a self,
        path: &'a Path,
        content: &'a [u8],
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<(), EnvError>>;

    /// 获取文件或目录的元数据。
    fn file_info<'a>(
        &'a self,
        path: &'a Path,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<FileInfo, EnvError>>;

    /// 列出目录内容。
    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, EnvError>>;

    /// 检查路径是否存在。
    fn exists<'a>(
        &'a self,
        path: &'a Path,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<bool, EnvError>>;

    /// 创建目录；`recursive` 对应 `mkdir -p`。
    fn create_dir<'a>(
        &'a self,
        path: &'a Path,
        recursive: bool,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<(), EnvError>>;

    /// 删除文件或目录；`recursive` 对应 `rm -r`，`force` 对应 `rm -f`。
    fn remove<'a>(
        &'a self,
        path: &'a Path,
        recursive: bool,
        force: bool,
        abort: CancellationToken,
    ) -> BoxFuture<'a, Result<(), EnvError>>;

    /// 创建临时目录；返回其绝对路径。
    fn create_temp_dir<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxFuture<'a, Result<PathBuf, EnvError>>;

    /// 执行 shell 命令。
    fn execute_shell<'a>(
        &'a self,
        cmd: &'a str,
        opts: ShellOptions<'a>,
    ) -> BoxFuture<'a, Result<ShellOutput, EnvError>>;

    /// 释放 env 持有的临时资源（best-effort）。
    fn cleanup<'a>(&'a self) -> BoxFuture<'a, Result<(), EnvError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_output_fields() {
        let o = ShellOutput {
            stdout: "hello".into(),
            stderr: "".into(),
            exit_code: 0,
        };
        assert_eq!(o.exit_code, 0);
    }

    #[test]
    fn file_info_is_dir() {
        let fi = FileInfo {
            path: std::path::PathBuf::from("/tmp"),
            is_dir: true,
            size: 0,
            modified: chrono::Utc::now(),
        };
        assert!(fi.is_dir);
    }

    #[test]
    fn execution_env_is_object_safe() {
        fn _accepts(_: &dyn ExecutionEnv) {}
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types env
```

Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/env.rs
git commit -m "feat(types): implement ExecutionEnv trait and supporting types"
```

---

### Task 9: Miscellaneous types (ThinkingLevel, AgentContext, TurnSnapshot, StreamOptions)

**Files:**
- Create: `crates/llm-harness-types/src/misc.rs`
- Create: `crates/llm-harness-types/src/resources.rs`

- [ ] **Step 1: Write failing tests**

```rust
// In misc.rs tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_level_copy() {
        let l = ThinkingLevel::High;
        let l2 = l; // copy
        assert!(matches!(l2, ThinkingLevel::High));
    }

    #[test]
    fn agent_context_default_no_system_prompt() {
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        assert!(ctx.system_prompt.is_none());
    }

    #[test]
    fn stream_options_default() {
        let opts = StreamOptions::default();
        assert!(opts.timeout_ms.is_none());
        assert!(opts.max_retries.is_none());
        assert!(opts.headers.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types misc 2>&1 | head -20
```

Expected: FAIL — types not defined.

- [ ] **Step 3: Implement misc.rs**

```rust
use std::sync::Arc;

use crate::{AgentMessage, Tool};

/// Provider 推理深度级别。
///
/// 各 provider 的实际映射由 `llm-api-adapter` 负责（如 Anthropic → `budget_tokens`，
/// OpenAI → `reasoning_effort`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingLevel {
    /// 禁用推理（节省 token）。
    Off,
    /// 最小推理深度。
    Minimal,
    /// 低推理深度。
    Low,
    /// 中等推理深度。
    Medium,
    /// 高推理深度。
    High,
    /// 最高推理深度（仅部分模型支持）。
    XHigh,
}

/// Agent loop 的输入上下文。
pub struct AgentContext {
    /// 系统提示；`None` 表示不设置系统提示。
    pub system_prompt: Option<String>,
    /// 消息历史列表。
    pub messages: Vec<AgentMessage>,
}

/// Turn 开始时的配置快照；turn 进行中对 Agent 的修改不影响当前 turn。
#[derive(Clone)]
pub struct TurnSnapshot {
    /// 使用的模型 ID。
    pub model: String,
    /// 推理深度级别。
    pub thinking_level: ThinkingLevel,
    /// 本 turn 激活的工具列表。
    pub tools: Vec<Arc<dyn Tool>>,
    /// 系统提示。
    pub system_prompt: Option<String>,
}

/// 传递给 LLM provider 的传输层配置；可被 `BeforeProviderRequestHook` 覆盖。
#[derive(Debug, Clone, Default)]
pub struct StreamOptions {
    /// 请求超时（毫秒）；`None` 表示无超时。
    pub timeout_ms: Option<u64>,
    /// 最大重试次数；`None` 表示使用 provider 默认值。
    pub max_retries: Option<u32>,
    /// 重试最大延迟（毫秒）；`None` 表示使用 provider 默认值。
    pub max_retry_delay_ms: Option<u64>,
    /// 附加的 HTTP 请求头。
    pub headers: Vec<(String, String)>,
    /// 厂商特定的元数据（透传给 provider）。
    pub metadata: serde_json::Value,
    /// 厂商特定的缓存配置（透传给 provider）。
    pub cache_config: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_level_copy() {
        let l = ThinkingLevel::High;
        let l2 = l;
        assert!(matches!(l2, ThinkingLevel::High));
    }

    #[test]
    fn agent_context_default_no_system_prompt() {
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        assert!(ctx.system_prompt.is_none());
    }

    #[test]
    fn stream_options_default() {
        let opts = StreamOptions::default();
        assert!(opts.timeout_ms.is_none());
        assert!(opts.max_retries.is_none());
        assert!(opts.headers.is_empty());
    }
}
```

- [ ] **Step 4: Implement resources.rs (AgentHarnessResources stub)**

`BeforeRunCtx` in `hooks.rs` references `AgentHarnessResources`. Define a stub here:

```rust
/// Harness 运行时资源——由 `BeforeRunHook` 的上下文携带，供 hook 访问。
///
/// 具体字段在 Phase 7（AgentHarness）中填充；此处为 stub，
/// 使 types crate 中的 hook trait 可以编译。
pub struct AgentHarnessResources {
    _private: (),
}
```

Save to: `crates/llm-harness-types/src/resources.rs`

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types misc
```

Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/llm-harness-types/src/misc.rs crates/llm-harness-types/src/resources.rs
git commit -m "feat(types): implement ThinkingLevel, AgentContext, TurnSnapshot, StreamOptions"
```

---

### Task 10: Hook traits

**Files:**
- Create: `crates/llm-harness-types/src/hooks.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn before_tool_call_decision_allow() {
        let d = BeforeToolCallDecision::Allow;
        assert!(matches!(d, BeforeToolCallDecision::Allow));
    }

    #[test]
    fn after_tool_call_decision_passthrough() {
        let d = AfterToolCallDecision::Passthrough;
        assert!(matches!(d, AfterToolCallDecision::Passthrough));
    }

    #[test]
    fn tool_result_patch_all_none() {
        let p = ToolResultPatch {
            content: None,
            details: None,
            is_error: None,
            terminate: None,
        };
        assert!(p.content.is_none());
    }

    #[test]
    fn before_compact_decision_skip() {
        let d = BeforeCompactDecision::Skip;
        assert!(matches!(d, BeforeCompactDecision::Skip));
    }

    #[test]
    fn auth_info_fields() {
        let a = AuthInfo {
            api_key: Some("sk-test".into()),
            headers: vec![("X-Custom".into(), "val".into())],
        };
        assert!(a.api_key.is_some());
        assert_eq!(a.headers.len(), 1);
    }

    #[test]
    fn all_hook_traits_are_object_safe() {
        fn _a(_: &dyn TransformContextHook) {}
        fn _b(_: &dyn PrepareNextTurnHook) {}
        fn _c(_: &dyn BeforeToolCallHook) {}
        fn _d(_: &dyn AfterToolCallHook) {}
        fn _e(_: &dyn ShouldStopHook) {}
        fn _f(_: &dyn BeforeProviderRequestHook) {}
        fn _g(_: &dyn AfterProviderResponseHook) {}
        fn _h(_: &dyn AuthHook) {}
        fn _i(_: &dyn BeforeRunHook) {}
        fn _j(_: &dyn BeforeTurnHook) {}
        fn _k(_: &dyn AfterTurnHook) {}
        fn _l(_: &dyn BeforeCompactHook) {}
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p llm-harness-types hooks 2>&1 | head -20
```

Expected: FAIL — hook types not defined.

- [ ] **Step 3: Implement hooks.rs**

```rust
use std::{collections::HashSet, sync::Arc};

use futures::future::BoxFuture;

use crate::{
    AgentContext, AgentError, AgentHarnessResources, AgentMessage, AssistantMessage,
    CompactionResult, ContentBlock, StopReason, StreamOptions, Tool, ToolError, ToolResult,
    TurnSnapshot,
};

// ── TransformContextHook ──────────────────────────────────────────────────────

/// 每次 LLM 调用前对上下文做转换；compaction 通过此 hook 接入。
pub trait TransformContextHook: Send + Sync {
    /// 对 `ctx` 做变换后返回新的上下文。
    fn transform<'a>(
        &'a self,
        ctx: AgentContext,
    ) -> BoxFuture<'a, Result<AgentContext, AgentError>>;
}

// ── PrepareNextTurnHook ───────────────────────────────────────────────────────

/// 传递给 `PrepareNextTurnHook::prepare` 的上下文。
pub struct PrepareNextTurnCtx<'a> {
    /// 当前 turn 编号（从 0 开始）。
    pub turn_index: u32,
    /// 上一轮的完整 LLM 回复。
    pub last_message: &'a AssistantMessage,
    /// 上一轮所有 tool 执行结果；key 为 `tool_use_id`。
    pub last_tool_results: &'a [(String, Result<ToolResult, ToolError>)],
}

/// `PrepareNextTurnHook::prepare` 的返回值；`None` 字段表示沿用当前值。
pub struct NextTurnDirective {
    /// 替换下一轮的完整上下文；`None` 表示沿用当前上下文。
    pub context: Option<AgentContext>,
    /// 替换下一轮使用的模型 ID；`None` 表示沿用当前模型。
    pub model: Option<String>,
    /// 替换下一轮的推理深度；`None` 表示沿用当前级别。
    pub thinking_level: Option<crate::ThinkingLevel>,
    /// 替换全部工具列表；`None` 表示沿用当前工具列表。
    pub tools: Option<Vec<Arc<dyn Tool>>>,
    /// 仅控制激活工具子集（在当前或已替换的全集中过滤）；`None` 表示激活全部。
    pub active_tools: Option<HashSet<String>>,
}

/// 每个 turn 结束后调用，返回下一轮的配置。
pub trait PrepareNextTurnHook: Send + Sync {
    /// 根据上一轮结果决定下一轮配置。
    fn prepare<'a>(
        &'a self,
        ctx: PrepareNextTurnCtx<'a>,
    ) -> BoxFuture<'a, Result<NextTurnDirective, AgentError>>;
}

// ── BeforeToolCallHook ────────────────────────────────────────────────────────

/// 传递给 `BeforeToolCallHook::on_call` 的上下文。
pub struct BeforeToolCallCtx<'a> {
    /// 触发本次 tool call 的 LLM 回复。
    pub assistant_message: &'a AssistantMessage,
    /// 当前 tool call 的唯一 ID。
    pub tool_use_id: &'a str,
    /// 工具名称。
    pub tool_name: &'a str,
    /// 工具调用参数。
    pub args: &'a serde_json::Value,
    /// 当前轮次索引。
    pub turn_index: u32,
}

/// `BeforeToolCallHook::on_call` 的返回决策。
pub enum BeforeToolCallDecision {
    /// 允许工具按原参数执行。
    Allow,
    /// 以修改后的参数执行工具。
    Modify(serde_json::Value),
    /// 拒绝执行，直接返回指定的 `ToolResult`。
    Deny(ToolResult),
}

/// 工具执行前的拦截 hook。
pub trait BeforeToolCallHook: Send + Sync {
    /// 在工具执行前决定是否允许、修改参数或拒绝执行。
    fn on_call<'a>(
        &'a self,
        ctx: BeforeToolCallCtx<'a>,
    ) -> BoxFuture<'a, BeforeToolCallDecision>;
}

// ── AfterToolCallHook ─────────────────────────────────────────────────────────

/// 传递给 `AfterToolCallHook::on_complete` 的上下文。
pub struct AfterToolCallCtx<'a> {
    /// 触发本次 tool call 的 LLM 回复。
    pub assistant_message: &'a AssistantMessage,
    /// 当前 tool call 的唯一 ID。
    pub tool_use_id: &'a str,
    /// 工具名称。
    pub tool_name: &'a str,
    /// 工具调用参数。
    pub args: &'a serde_json::Value,
    /// 工具执行结果。
    pub result: &'a Result<ToolResult, ToolError>,
    /// 当前轮次索引。
    pub turn_index: u32,
}

/// `ToolResult` 的部分覆盖补丁；`None` 字段表示保持原值。
pub struct ToolResultPatch {
    /// 覆盖内容块列表。
    pub content: Option<Vec<ContentBlock>>,
    /// 覆盖扩展数据。
    pub details: Option<serde_json::Value>,
    /// 覆盖错误标志。
    pub is_error: Option<bool>,
    /// 覆盖终止标志。
    pub terminate: Option<bool>,
}

/// `AfterToolCallHook::on_complete` 的返回决策。
pub enum AfterToolCallDecision {
    /// 照常使用工具执行结果，不做修改。
    Passthrough,
    /// 部分覆盖执行结果。
    Patch(ToolResultPatch),
}

/// 工具执行后的结果拦截 hook。
pub trait AfterToolCallHook: Send + Sync {
    /// 在工具执行完成后决定是否覆盖结果。
    fn on_complete<'a>(
        &'a self,
        ctx: AfterToolCallCtx<'a>,
    ) -> BoxFuture<'a, AfterToolCallDecision>;
}

// ── ShouldStopHook ────────────────────────────────────────────────────────────

/// 传递给 `ShouldStopHook::should_stop` 的上下文。
pub struct ShouldStopCtx<'a> {
    /// 最后一条 LLM 回复。
    pub last_assistant: &'a AssistantMessage,
    /// LLM 停止原因。
    pub stop_reason: StopReason,
    /// 当前轮次索引。
    pub turn_index: u32,
}

/// LLM 自然停止后的继续决策 hook。
///
/// 返回 `true` 停止 loop；返回 `false` 强制再跑一轮（适用于 `MaxTokens` 等截断场景）。
/// 不能用于中断进行中的 turn——中断走 `abort()`。
pub trait ShouldStopHook: Send + Sync {
    /// 仅在 LLM 自然停止时调用；返回 `true` 才停止。
    fn should_stop<'a>(&'a self, ctx: ShouldStopCtx<'a>) -> BoxFuture<'a, bool>;
}

// ── Provider Request/Response Hooks ───────────────────────────────────────────

/// LLM provider 请求前拦截 hook；可原地修改传输层配置。
pub trait BeforeProviderRequestHook: Send + Sync {
    /// 在 LLM 调用前修改 `StreamOptions`（可修改 timeout、headers 等）。
    fn before_request<'a>(&'a self, opts: &'a mut StreamOptions) -> BoxFuture<'a, ()>;
}

/// Provider 响应的元数据信息。
pub struct ProviderResponseInfo {
    /// HTTP 状态码；`None` 表示流式请求未携带状态码。
    pub status_code: Option<u16>,
    /// HTTP 响应头。
    pub response_headers: Vec<(String, String)>,
    /// Token 用量；`None` 表示 provider 未返回。
    pub usage: Option<crate::TokenUsage>,
    /// 请求延迟（毫秒）。
    pub latency_ms: u64,
}

/// LLM provider 响应后的观测 hook；用于配额追踪、成本监控等。
pub trait AfterProviderResponseHook: Send + Sync {
    /// 在收到 provider 响应后调用（纯观测，无返回值）。
    fn after_response<'a>(&'a self, info: &'a ProviderResponseInfo) -> BoxFuture<'a, ()>;
}

// ── AuthHook ──────────────────────────────────────────────────────────────────

/// 动态认证信息。
pub struct AuthInfo {
    /// API key；`None` 表示使用 provider 默认配置。
    pub api_key: Option<String>,
    /// 附加的认证 HTTP 头（如 OAuth token）。
    pub headers: Vec<(String, String)>,
}

/// 动态认证 hook；每次 LLM 调用前解析最新凭据（适用于 OAuth token 过期等场景）。
pub trait AuthHook: Send + Sync {
    /// 返回当前有效的认证信息。
    fn resolve<'a>(&'a self) -> BoxFuture<'a, Result<AuthInfo, AgentError>>;
}

// ── Turn 边界 Hooks ───────────────────────────────────────────────────────────

/// 传递给 `BeforeRunHook::before_run` 的上下文。
pub struct BeforeRunCtx<'a> {
    /// 用户输入的提示文本。
    pub prompt_text: &'a str,
    /// 本次运行注入的初始消息列表（可修改）。
    pub initial_messages: &'a mut Vec<AgentMessage>,
    /// 系统提示（可修改）。
    pub system_prompt: &'a mut Option<String>,
    /// Harness 运行时资源。
    pub resources: &'a AgentHarnessResources,
}

/// `BeforeRunHook::before_run` 的返回值。
pub struct BeforeRunResult {
    /// 追加到 `initial_messages` 末尾的额外消息。
    pub additional_messages: Vec<AgentMessage>,
    /// 覆盖系统提示；`None` 表示沿用 `BeforeRunCtx` 中可能已修改的值。
    pub system_prompt: Option<String>,
}

/// Harness 专属：一次完整 agent 运行（prompt 调用）开始前的 hook。
pub trait BeforeRunHook: Send + Sync {
    /// 在 agent 开始运行前调用；可注入额外消息或修改系统提示。
    fn before_run<'a>(
        &'a self,
        ctx: BeforeRunCtx<'a>,
    ) -> BoxFuture<'a, Result<BeforeRunResult, AgentError>>;
}

/// 传递给 `BeforeTurnHook::before_turn` 的上下文。
pub struct BeforeTurnCtx<'a> {
    /// 当前 turn 编号（从 0 开始）。
    pub turn_index: u32,
    /// Turn 开始时的配置快照。
    pub snapshot: &'a TurnSnapshot,
}

/// 传递给 `AfterTurnHook::after_turn` 的上下文。
pub struct AfterTurnCtx<'a> {
    /// 当前 turn 编号（从 0 开始）。
    pub turn_index: u32,
    /// 本 turn 新增的消息列表。
    pub new_messages: &'a [AgentMessage],
}

/// Harness 专属：turn 开始前通知 hook（纯通知，无返回值）。
pub trait BeforeTurnHook: Send + Sync {
    /// Turn 开始前调用。
    fn before_turn<'a>(&'a self, ctx: BeforeTurnCtx<'a>) -> BoxFuture<'a, ()>;
}

/// Harness 专属：turn 结束后通知 hook（纯通知，无返回值）。
pub trait AfterTurnHook: Send + Sync {
    /// Turn 结束后调用。
    fn after_turn<'a>(&'a self, ctx: AfterTurnCtx<'a>) -> BoxFuture<'a, ()>;
}

// ── BeforeCompactHook ─────────────────────────────────────────────────────────

/// 传递给 `BeforeCompactHook::before_compact` 的上下文。
pub struct BeforeCompactCtx<'a> {
    /// 当前估算的 token 数。
    pub estimated_tokens: usize,
    /// 当前消息列表。
    pub messages: &'a [AgentMessage],
}

/// `BeforeCompactHook::before_compact` 的返回决策。
pub enum BeforeCompactDecision {
    /// 继续执行框架默认的 compaction 流程。
    Proceed,
    /// 跳过本次 compaction（由 hook 决定暂不压缩）。
    Skip,
    /// 使用 hook 提供的 `CompactionResult` 替代框架生成的摘要。
    Override(CompactionResult),
}

/// Compaction 执行前的决策 hook。
pub trait BeforeCompactHook: Send + Sync {
    /// 在 compaction 执行前决定是继续、跳过或使用自定义摘要。
    fn before_compact<'a>(
        &'a self,
        ctx: BeforeCompactCtx<'a>,
    ) -> BoxFuture<'a, BeforeCompactDecision>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn before_tool_call_decision_allow() {
        let d = BeforeToolCallDecision::Allow;
        assert!(matches!(d, BeforeToolCallDecision::Allow));
    }

    #[test]
    fn after_tool_call_decision_passthrough() {
        let d = AfterToolCallDecision::Passthrough;
        assert!(matches!(d, AfterToolCallDecision::Passthrough));
    }

    #[test]
    fn tool_result_patch_all_none() {
        let p = ToolResultPatch {
            content: None,
            details: None,
            is_error: None,
            terminate: None,
        };
        assert!(p.content.is_none());
    }

    #[test]
    fn before_compact_decision_skip() {
        let d = BeforeCompactDecision::Skip;
        assert!(matches!(d, BeforeCompactDecision::Skip));
    }

    #[test]
    fn auth_info_fields() {
        let a = AuthInfo {
            api_key: Some("sk-test".into()),
            headers: vec![("X-Custom".into(), "val".into())],
        };
        assert!(a.api_key.is_some());
        assert_eq!(a.headers.len(), 1);
    }

    #[test]
    fn all_hook_traits_are_object_safe() {
        fn _a(_: &dyn TransformContextHook) {}
        fn _b(_: &dyn PrepareNextTurnHook) {}
        fn _c(_: &dyn BeforeToolCallHook) {}
        fn _d(_: &dyn AfterToolCallHook) {}
        fn _e(_: &dyn ShouldStopHook) {}
        fn _f(_: &dyn BeforeProviderRequestHook) {}
        fn _g(_: &dyn AfterProviderResponseHook) {}
        fn _h(_: &dyn AuthHook) {}
        fn _i(_: &dyn BeforeRunHook) {}
        fn _j(_: &dyn BeforeTurnHook) {}
        fn _k(_: &dyn AfterTurnHook) {}
        fn _l(_: &dyn BeforeCompactHook) {}
    }
}
```

Note: `hooks.rs` uses `CompactionResult` which is a type that will be defined in Phase 5. For Phase 1 to compile, add a placeholder stub in `errors.rs` or a new file:

```rust
// In a new file: crates/llm-harness-types/src/compaction.rs
// or appended to errors.rs as a stub

/// Compaction 结果——由 Phase 5 完整定义；此处为 Phase 1 的 stub。
pub struct CompactionResult {
    /// 摘要文本。
    pub summary: String,
    /// 压缩后保留的第一条 entry 的 ID（None = 全量压缩）。
    pub first_kept_entry_id: Option<crate::EntryId>,
}
```

Add `pub mod compaction;` and `pub use compaction::*;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p llm-harness-types hooks
cargo test -p llm-harness-types
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/llm-harness-types/src/hooks.rs crates/llm-harness-types/src/compaction.rs crates/llm-harness-types/src/lib.rs
git commit -m "feat(types): implement all hook traits and CompactionResult stub"
```

---

### Task 11: Final verification

- [ ] **Step 1: Run all tests**

```bash
cargo test -p llm-harness-types
```

Expected: all tests pass, zero failures.

- [ ] **Step 2: Check no std::io or network in types crate**

```bash
grep -rn "std::fs\|std::net\|tokio::fs\|reqwest\|hyper" crates/llm-harness-types/src/
```

Expected: no output (zero IO in types crate).

- [ ] **Step 3: Verify dependency rule — no external crates except the allowed list**

```bash
cargo tree -p llm-harness-types --depth 1
```

Expected output contains only: `anyhow`, `chrono`, `futures`, `serde`, `serde_json`, `thiserror`, `tokio`, `tokio-util`, `uuid`. No `reqwest`, no `llm_adapter`.

- [ ] **Step 4: Run clippy**

```bash
cargo clippy -p llm-harness-types -- -D warnings
```

Expected: no warnings.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "chore(types): phase 1 complete — all types, traits, tests passing"
```
