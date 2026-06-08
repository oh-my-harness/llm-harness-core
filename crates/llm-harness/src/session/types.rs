use chrono::{DateTime, Utc};
use llm_harness_types::*;
use serde::{Deserialize, Serialize};

// ── SessionEntryPayload ────────────────────────────────────────────────────────

/// All possible payloads stored in a session log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry_type", rename_all = "snake_case")]
pub enum SessionEntryPayload {
    /// An agent message (user, assistant, tool-result, etc.). 90%+ of all entries.
    Message(AgentMessage),
    /// Model configuration change; replays via `build_context` to restore last model.
    ModelChange {
        to: String,
        provider: Option<String>,
        model_id: Option<String>,
    },
    /// Thinking-level change.
    ThinkingLevelChange { to: ThinkingLevel },
    /// Active tool list change.
    ActiveToolsChange { active: Vec<String> },
    /// A compaction event: summary message + first kept entry reference.
    Compaction(CompactionEntry),
    /// A named label marking an interesting point in the tree (for navigation).
    Label { name: String },
    /// A session naming event; latest one wins.
    SessionInfo { name: String },
    /// Application-layer custom entry.
    Custom {
        /// Application-defined sub-type tag.
        #[serde(rename = "type")]
        custom_type: String,
        data: serde_json::Value,
    },
    /// Semantic annotation that a branch was created here (optional; for UI).
    BranchPoint {
        from: EntryId,
        label: Option<String>,
    },
    /// Records a cursor switch between two branches.
    BranchSwitch {
        from: EntryId,
        to: EntryId,
        summary: Option<String>,
    },
    /// AI-generated summary of a branch.
    BranchSummary(BranchSummaryEntry),
}

/// Compaction data stored in a session entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEntry {
    /// The summary message inserted into context after compaction.
    pub summary_message: AgentMessage,
    /// First entry still valid after compaction; everything before is covered by the summary.
    pub first_kept_entry: EntryId,
    /// Estimated token count before compaction (UI display).
    pub tokens_before: usize,
    /// True if the summary came from a `BeforeCompactHook` rather than the framework.
    pub from_hook: bool,
    /// Opaque extension data (e.g. modified files list).
    pub details: Option<serde_json::Value>,
}

/// AI-generated branch summary entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryEntry {
    /// The leaf entry ID of the summarized branch.
    pub leaf_id: EntryId,
    /// First entry covered by the summary (inclusive).
    pub from_entry: EntryId,
    /// AI-generated summary text.
    pub summary: String,
    /// Estimated token count of the summarized range.
    pub token_count: usize,
}

// ── SessionEntry ───────────────────────────────────────────────────────────────

/// A single node in the session tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    /// Unique entry ID (UUIDv7, time-ordered).
    pub id: EntryId,
    /// Parent entry ID; `None` means this is the tree root.
    pub parent_id: Option<EntryId>,
    /// Creation timestamp.
    pub timestamp: DateTime<Utc>,
    /// Typed payload.
    pub payload: SessionEntryPayload,
}

// ── Session metadata ───────────────────────────────────────────────────────────

/// Persistent metadata for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session identifier (stable across renames).
    pub id: String,
    /// Human-readable session name (most recent `SessionInfo` entry wins).
    pub name: Option<String>,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// When the session was last modified.
    pub updated_at: DateTime<Utc>,
    /// Last known model ID.
    pub model: Option<String>,
    /// Current write cursor; `None` = no entries yet.
    pub active_cursor: Option<EntryId>,
    /// Source session path for cross-session forks (reference mode, v1.x only).
    pub parent_session_path: Option<String>,
}

// ── Creation / list options ────────────────────────────────────────────────────

/// Options for creating a new session.
#[derive(Debug, Default)]
pub struct CreateSessionOptions {
    pub name: Option<String>,
    pub initial_model: Option<String>,
    pub initial_thinking_level: Option<ThinkingLevel>,
    pub initial_tools: Vec<String>,
}

/// Options for listing sessions.
#[derive(Debug, Default)]
pub struct ListSessionOptions {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub order: ListOrder,
    pub name_contains: Option<String>,
}

/// Sort order for session listings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ListOrder {
    #[default]
    UpdatedDesc,
    UpdatedAsc,
    CreatedDesc,
    CreatedAsc,
}

/// Options for cross-session fork.
#[derive(Debug)]
pub struct ForkOptions {
    pub name: Option<String>,
    /// v1 forces this to `true` (full entry copy with new IDs).
    pub copy_entries: bool,
}

impl Default for ForkOptions {
    fn default() -> Self {
        Self {
            name: None,
            copy_entries: true,
        }
    }
}

// ── Branch info ────────────────────────────────────────────────────────────────

/// Summary information about a single branch (leaf → root path).
#[derive(Debug, Clone)]
pub struct BranchInfo {
    /// The leaf entry ID of this branch.
    pub leaf_id: EntryId,
    /// Optional branch label.
    pub label: Option<String>,
    /// Number of entries from root to this leaf.
    pub message_count: usize,
    /// Timestamp of the most recent entry.
    pub last_activity: DateTime<Utc>,
    /// AI-generated branch summary, if available.
    pub summary: Option<String>,
}

// ── BuiltContext ───────────────────────────────────────────────────────────────

/// The "effective context" built from the active branch's entries.
///
/// Compaction entries are resolved: messages before the last compaction are
/// replaced with the compaction summary message.
#[derive(Debug)]
pub struct BuiltContext {
    /// Effective message list to pass to the LLM.
    pub messages: Vec<AgentMessage>,
    /// Last known model ID from the session log.
    pub last_model: Option<String>,
    /// Last known thinking level from the session log.
    pub last_thinking_level: Option<ThinkingLevel>,
    /// Last known active tools list from the session log.
    pub last_active_tools: Option<Vec<String>>,
}

// ── SessionEntryKind ───────────────────────────────────────────────────────────

/// Discriminant for `find_entries_by_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEntryKind {
    Message,
    ModelChange,
    ThinkingLevelChange,
    ActiveToolsChange,
    Compaction,
    Label,
    SessionInfo,
    Custom,
    BranchPoint,
    BranchSwitch,
    BranchSummary,
}

impl SessionEntry {
    /// Returns the `SessionEntryKind` for this entry.
    pub fn kind(&self) -> SessionEntryKind {
        match &self.payload {
            SessionEntryPayload::Message(_) => SessionEntryKind::Message,
            SessionEntryPayload::ModelChange { .. } => SessionEntryKind::ModelChange,
            SessionEntryPayload::ThinkingLevelChange { .. } => {
                SessionEntryKind::ThinkingLevelChange
            }
            SessionEntryPayload::ActiveToolsChange { .. } => SessionEntryKind::ActiveToolsChange,
            SessionEntryPayload::Compaction(_) => SessionEntryKind::Compaction,
            SessionEntryPayload::Label { .. } => SessionEntryKind::Label,
            SessionEntryPayload::SessionInfo { .. } => SessionEntryKind::SessionInfo,
            SessionEntryPayload::Custom { .. } => SessionEntryKind::Custom,
            SessionEntryPayload::BranchPoint { .. } => SessionEntryKind::BranchPoint,
            SessionEntryPayload::BranchSwitch { .. } => SessionEntryKind::BranchSwitch,
            SessionEntryPayload::BranchSummary(_) => SessionEntryKind::BranchSummary,
        }
    }
}
