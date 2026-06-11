use chrono::Utc;
use llm_harness_loop::LlmMessage;
use llm_harness_loop::{ChatRequest, LlmClient, RequestContent};
use llm_harness_types::*;

use crate::agent::ModelInfo;
use crate::session::types::{CompactionEntry, SessionEntry, SessionEntryPayload};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Tokens estimated per character (rough 4-char-per-token approximation).
const CHARS_PER_TOKEN: usize = 4;

const SUMMARIZATION_SYSTEM_PROMPT: &str = "\
You are a context summarization assistant. Your task is to read a conversation \
between a user and an AI coding assistant, then produce a structured summary \
that another LLM instance will use to continue the work seamlessly.\n\
\n\
Be concise but complete. Preserve:\n\
- What the user is trying to accomplish\n\
- What has been done and what remains\n\
- Key decisions and their rationale\n\
- Critical technical details (file paths, function names, error messages)\n\
- The immediate next steps\n";

const SUMMARY_REQUEST: &str = "\
Create a structured context checkpoint summary using this format:\n\
\n\
## Goal\n\
[What is the user trying to accomplish?]\n\
\n\
## Progress\n\
### Done\n\
- [x] [Completed tasks]\n\
### In Progress\n\
- [ ] [Current work]\n\
\n\
## Key Decisions\n\
- **[Decision]**: [Brief rationale]\n\
\n\
## Next Steps\n\
1. [Ordered list]\n\
\n\
## Critical Context\n\
- [File paths, function names, error messages to preserve]\n";

// ── CompactionSettings ────────────────────────────────────────────────────────

/// Configuration for the compaction subsystem.
#[derive(Debug, Clone)]
pub struct CompactionSettings {
    /// Whether automatic compaction is enabled.
    pub enabled: bool,
    /// Tokens reserved for LLM response headroom; compaction triggers when
    /// `total_tokens > context_window - reserve_tokens`.
    pub reserve_tokens: usize,
    /// Tokens of recent history to preserve uncompressed.
    pub keep_recent_tokens: usize,
    /// Model ID used to generate the summary.
    pub summary_model: String,
    /// Metadata for the summary model (used for token budget checks).
    pub summary_model_info: ModelInfo,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            summary_model: String::new(),
            summary_model_info: ModelInfo {
                context_window: 200_000,
                max_tokens: 8192,
            },
        }
    }
}

// ── CompactionPreparation ─────────────────────────────────────────────────────

/// Intermediate result of `prepare_compaction`: decision + cut-point data.
///
/// Separates the pure decision logic from the async LLM summarization call.
pub struct CompactionPreparation {
    /// Full path entries from root to active leaf.
    pub path_entries: Vec<SessionEntry>,
    /// First entry still valid from the previous compaction (or session root).
    pub first_kept_entry: EntryId,
    /// Entry at which the new compaction starts keeping content.
    pub cut_point: EntryId,
    /// Existing summary text (from last compaction) for iterative update mode.
    pub previous_summary: Option<String>,
    /// Estimated total token count before this compaction.
    pub estimated_tokens: usize,
    /// Entries that were split across the cut point (v1: always None).
    pub split_turn_prefix: Option<Vec<SessionEntry>>,
    /// File operations accumulated during the compacted period.
    pub file_operations: Vec<FileOperation>,
}

// ── Token estimation ──────────────────────────────────────────────────────────

fn estimate_tokens_for_text(s: &str) -> usize {
    (s.len() / CHARS_PER_TOKEN).max(1)
}

fn estimate_tokens_for_content_blocks(blocks: &[ContentBlock]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => estimate_tokens_for_text(text),
            ContentBlock::Image { .. } => 250,
            ContentBlock::Thinking { thinking, .. } => estimate_tokens_for_text(thinking),
            ContentBlock::ToolUse { input, .. } => estimate_tokens_for_text(&input.to_string()),
        })
        .sum()
}

fn estimate_tokens_for_message(msg: &AgentMessage) -> usize {
    match msg {
        AgentMessage::User(u) => estimate_tokens_for_content_blocks(&u.content),
        AgentMessage::Assistant(a) => {
            // Use actual output token count when available; `input_tokens` reflects the
            // full request cost (all prior context), not the size of this message alone.
            if let Some(usage) = &a.usage {
                return usage.output_tokens as usize;
            }
            estimate_tokens_for_content_blocks(&a.content)
        }
        AgentMessage::ToolResult(t) => estimate_tokens_for_content_blocks(&t.content),
        AgentMessage::CompactionSummary(cs) => estimate_tokens_for_text(&cs.summary),
        AgentMessage::BranchSummary(bs) => estimate_tokens_for_text(&bs.summary),
        AgentMessage::Custom(c) => estimate_tokens_for_text(&c.data.to_string()),
    }
}

/// Estimates the token cost of a single session entry.
///
/// Uses actual usage data when available, falls back to character-count heuristics.
/// Non-message entries (config changes) contribute zero tokens.
pub(crate) fn estimate_tokens_for_entry(entry: &SessionEntry) -> usize {
    match &entry.payload {
        SessionEntryPayload::Message(msg) => estimate_tokens_for_message(msg),
        // Config changes contribute negligible tokens.
        _ => 0,
    }
}

// ── prepare_compaction ────────────────────────────────────────────────────────

/// Decide whether compaction is needed and determine the cut point.
///
/// Returns `None` when the current token count is within budget.
/// Pure function — does not call the LLM.
pub fn prepare_compaction(
    path: &[SessionEntry],
    last_compaction: Option<&CompactionEntry>,
    settings: &CompactionSettings,
    model_info: &ModelInfo,
) -> Option<CompactionPreparation> {
    if !settings.enabled || path.is_empty() {
        return None;
    }

    // Determine where the compactable region starts.
    let start_idx = last_compaction
        .and_then(|lc| path.iter().position(|e| e.id == lc.first_kept_entry))
        .unwrap_or(0);

    let compactable = &path[start_idx..];

    // Estimate total token cost.
    let estimated_tokens: usize = compactable.iter().map(estimate_tokens_for_entry).sum();

    let threshold = (model_info.context_window as usize).saturating_sub(settings.reserve_tokens);
    if estimated_tokens <= threshold {
        return None;
    }

    // Scan backward to find the first entry we need to keep.
    let mut kept_tokens: usize = 0;
    let mut cut_relative = compactable.len(); // index within compactable where kept section starts

    for (i, entry) in compactable.iter().enumerate().rev() {
        let t = estimate_tokens_for_entry(entry);
        if kept_tokens + t > settings.keep_recent_tokens && i + 1 < compactable.len() {
            cut_relative = i + 1;
            break;
        }
        kept_tokens += t;
        if i == 0 {
            // Even keeping everything doesn't exceed threshold; can't compact.
            return None;
        }
    }

    // Adjust cut_relative backward to land on a UserMessage boundary.
    let valid_cut = find_valid_cut_boundary(compactable, cut_relative);
    let valid_cut = valid_cut?; // None = no valid boundary found

    let first_kept_entry = compactable[valid_cut].id;
    let cut_point = first_kept_entry;

    // Extract previous summary from last compaction for iterative update.
    let previous_summary = last_compaction.and_then(|lc| {
        if let AgentMessage::CompactionSummary(cs) = &lc.summary_message {
            Some(cs.summary.clone())
        } else {
            None
        }
    });

    // The `first_kept_entry` field in the returned Preparation tracks what's
    // already been summarised (from the last compaction), not the new cut point.
    let prev_first_kept = last_compaction
        .map(|lc| lc.first_kept_entry)
        .unwrap_or(path[0].id);

    Some(CompactionPreparation {
        path_entries: path.to_vec(),
        first_kept_entry: prev_first_kept,
        cut_point,
        previous_summary,
        estimated_tokens,
        split_turn_prefix: None,
        file_operations: vec![],
    })
}

/// Find the nearest index at or before `cut_relative` in `entries` that is a
/// valid turn boundary (a `UserMessage` or `CompactionSummary`).
///
/// Returns `None` if no valid boundary exists in the range.
fn find_valid_cut_boundary(entries: &[SessionEntry], cut_relative: usize) -> Option<usize> {
    // Scan backward from cut_relative (inclusive) to find a UserMessage.
    (0..=cut_relative.min(entries.len().saturating_sub(1)))
        .rev()
        .find(|&i| is_valid_cut_start(&entries[i]))
}

fn is_valid_cut_start(entry: &SessionEntry) -> bool {
    matches!(
        &entry.payload,
        SessionEntryPayload::Message(AgentMessage::User(_))
            | SessionEntryPayload::Message(AgentMessage::CompactionSummary(_))
            | SessionEntryPayload::Message(AgentMessage::BranchSummary(_))
    )
}

// ── compact ───────────────────────────────────────────────────────────────────

/// Call the summary LLM to generate a compaction summary.
///
/// Does **not** write to the session; the caller (Harness) appends the result.
/// If the auth hook is provided it is accepted but v1 ignores it — auth must be
/// configured at `client` construction time.
pub async fn compact(
    client: &(dyn LlmClient + Send + Sync),
    preparation: CompactionPreparation,
    settings: &CompactionSettings,
    _auth: Option<&(dyn AuthHook + Send + Sync)>,
) -> Result<CompactionResult, CompactionError> {
    // Find the cut point index in path_entries.
    let cut_idx = preparation
        .path_entries
        .iter()
        .position(|e| e.id == preparation.cut_point)
        .unwrap_or(preparation.path_entries.len());

    // For second+ compaction, only compress entries since the previous kept boundary,
    // not the entire path (those earlier entries are already captured in previous_summary).
    let first_kept_idx = preparation
        .path_entries
        .iter()
        .position(|e| e.id == preparation.first_kept_entry)
        .unwrap_or(0);

    let entries_to_compress = &preparation.path_entries[first_kept_idx..cut_idx];

    // Format conversation history as text.
    let conversation_text = format_entries_as_text(entries_to_compress);

    // Build user message.
    let user_content = if let Some(prev) = &preparation.previous_summary {
        format!(
            "The messages below are NEW conversation messages to incorporate into \
            the existing summary provided in <previous-summary> tags.\n\n\
            <previous-summary>\n{prev}\n</previous-summary>\n\n\
            New conversation:\n<conversation>\n{conversation_text}\n</conversation>\n\n\
            Update the summary to incorporate the new information. Preserve existing \
            completed items, update in-progress items, add new progress.\n\n{SUMMARY_REQUEST}"
        )
    } else {
        format!(
            "Here is the conversation to summarize:\n\n\
            <conversation>\n{conversation_text}\n</conversation>\n\n\
            {SUMMARY_REQUEST}"
        )
    };

    // Validate that the prompt fits within the summary model's context window.
    let prompt_tokens = estimate_tokens_for_text(SUMMARIZATION_SYSTEM_PROMPT)
        + estimate_tokens_for_text(&user_content);
    let max_input_tokens = (settings.summary_model_info.context_window as usize)
        .saturating_sub(settings.summary_model_info.max_tokens as usize);
    if prompt_tokens > max_input_tokens {
        return Err(CompactionError::SummaryFailed(format!(
            "conversation too long to summarize: ~{prompt_tokens} tokens exceeds \
             summary model input budget of {max_input_tokens}"
        )));
    }

    let req = ChatRequest::builder(
        settings.summary_model.clone(),
        settings.summary_model_info.max_tokens,
    )
    .messages(vec![
        LlmMessage::System(SUMMARIZATION_SYSTEM_PROMPT.to_string()),
        LlmMessage::User(vec![RequestContent::Text(user_content)]),
    ])
    .build();

    let response = client
        .chat(&req)
        .await
        .map_err(|e| CompactionError::SummaryFailed(e.to_string()))?;

    let summary_text = response.text();
    if summary_text.trim().is_empty() {
        return Err(CompactionError::SummaryFailed(
            "empty summary response".into(),
        ));
    }

    // Append file operations summary if any.
    let full_summary = if preparation.file_operations.is_empty() {
        summary_text
    } else {
        let file_lines: Vec<String> = preparation
            .file_operations
            .iter()
            .map(|op| format!("- {:?}: {}", op.kind, op.path.display()))
            .collect();
        format!(
            "{summary_text}\n\n## Files Touched\n{}",
            file_lines.join("\n")
        )
    };

    let summary_message = AgentMessage::CompactionSummary(CompactionSummaryMessage {
        summary: full_summary,
        timestamp: Utc::now(),
    });

    // Estimate tokens_after = summary + kept entries.
    let summary_tokens = estimate_tokens_for_message(&summary_message);
    let kept_tokens: usize = preparation.path_entries[cut_idx..]
        .iter()
        .map(estimate_tokens_for_entry)
        .sum();
    let tokens_after = summary_tokens + kept_tokens;

    Ok(CompactionResult {
        summary_message,
        first_kept_entry: preparation.cut_point,
        tokens_before: preparation.estimated_tokens,
        tokens_after,
        file_operations: preparation.file_operations,
    })
}

// ── Conversation serialization ────────────────────────────────────────────────

/// Serializes session entries to a human-readable string for the compaction LLM.
///
/// Only message and model-change entries are rendered; other entry types are skipped.
pub(crate) fn format_entries_as_text(entries: &[SessionEntry]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for entry in entries {
        match &entry.payload {
            SessionEntryPayload::Message(msg) => {
                parts.push(format_message(msg));
            }
            SessionEntryPayload::ModelChange { to, .. } => {
                parts.push(format!("[Config] Model changed to: {to}"));
            }
            _ => {}
        }
    }

    parts.join("\n\n")
}

fn format_message(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::User(u) => {
            let text = content_blocks_to_text(&u.content);
            format!("[User]\n{text}")
        }
        AgentMessage::Assistant(a) => {
            let text = content_blocks_to_text(&a.content);
            format!("[Assistant]\n{text}")
        }
        AgentMessage::ToolResult(t) => {
            let text = content_blocks_to_text(&t.content);
            let status = if t.is_error { " (error)" } else { "" };
            format!("[Tool Result{status}]\n{text}")
        }
        AgentMessage::CompactionSummary(cs) => {
            format!("[Previous Summary]\n{}", cs.summary)
        }
        AgentMessage::BranchSummary(bs) => {
            format!("[Branch Summary]\n{}", bs.summary)
        }
        AgentMessage::Custom(c) => {
            format!("[Custom: {}]\n{}", c.r#type, c.data)
        }
    }
}

fn content_blocks_to_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::Thinking { thinking, .. } => thinking.clone(),
            ContentBlock::ToolUse { name, input, .. } => format!("[Tool: {name}({input})]"),
            ContentBlock::Image { .. } => "[image]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use llm_harness_types::*;

    use crate::session::types::{SessionEntry, SessionEntryPayload};

    use super::*;

    fn user_entry(id: EntryId, text: &str) -> SessionEntry {
        SessionEntry {
            id,
            parent_id: None,
            timestamp: Utc::now(),
            payload: SessionEntryPayload::Message(AgentMessage::User(UserMessage {
                content: vec![ContentBlock::Text { text: text.into() }],
                timestamp: Utc::now(),
            })),
        }
    }

    fn assistant_entry(id: EntryId, text: &str, tokens: Option<TokenUsage>) -> SessionEntry {
        SessionEntry {
            id,
            parent_id: None,
            timestamp: Utc::now(),
            payload: SessionEntryPayload::Message(AgentMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text { text: text.into() }],
                stop_reason: Some(StopReason::EndTurn),
                timestamp: Utc::now(),
                provider: None,
                api: None,
                model: None,
                usage: tokens,
                error_message: None,
            })),
        }
    }

    fn big_token_usage(n: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: 0,
            output_tokens: n,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }

    fn settings() -> CompactionSettings {
        CompactionSettings {
            enabled: true,
            reserve_tokens: 1000,
            keep_recent_tokens: 100,
            summary_model: "test-model".into(),
            summary_model_info: ModelInfo {
                context_window: 2000,
                max_tokens: 512,
            },
        }
    }

    fn model_info() -> ModelInfo {
        ModelInfo {
            context_window: 2000,
            max_tokens: 512,
        }
    }

    #[test]
    fn no_compaction_when_disabled() {
        let settings = CompactionSettings {
            enabled: false,
            ..settings()
        };
        let path = vec![user_entry(EntryId::new(), "hello")];
        assert!(prepare_compaction(&path, None, &settings, &model_info()).is_none());
    }

    #[test]
    fn no_compaction_when_under_threshold() {
        // total tokens estimated from small messages ≪ threshold (2000 - 1000 = 1000)
        let path = vec![
            user_entry(EntryId::new(), "hi"),
            assistant_entry(EntryId::new(), "hello", None),
        ];
        assert!(prepare_compaction(&path, None, &settings(), &model_info()).is_none());
    }

    #[test]
    fn compaction_triggered_when_over_threshold() {
        // Force large token counts via usage field.
        let path = vec![
            user_entry(EntryId::new(), "a"),
            assistant_entry(EntryId::new(), "b", Some(big_token_usage(900))),
            user_entry(EntryId::new(), "c"),
            assistant_entry(EntryId::new(), "d", Some(big_token_usage(900))),
            user_entry(EntryId::new(), "recent question"),
        ];
        let prep = prepare_compaction(&path, None, &settings(), &model_info());
        assert!(
            prep.is_some(),
            "should trigger compaction when over threshold"
        );
    }

    #[test]
    fn cut_point_lands_on_user_message() {
        let u1 = EntryId::new();
        let a1 = EntryId::new();
        let u2 = EntryId::new();
        let a2 = EntryId::new();
        let u3 = EntryId::new();
        let path = vec![
            user_entry(u1, "old question 1"),
            assistant_entry(a1, "answer 1", Some(big_token_usage(900))),
            user_entry(u2, "old question 2"),
            assistant_entry(a2, "answer 2", Some(big_token_usage(900))),
            user_entry(u3, "recent"),
        ];
        let prep = prepare_compaction(&path, None, &settings(), &model_info()).unwrap();
        // The cut_point should be a UserMessage entry.
        let cut_entry = path.iter().find(|e| e.id == prep.cut_point).unwrap();
        assert!(
            matches!(
                &cut_entry.payload,
                SessionEntryPayload::Message(AgentMessage::User(_))
            ),
            "cut point must be a user message"
        );
    }

    #[test]
    fn parse_command_args_basic() {
        use super::super::skills::parse_command_args;
        let args = parse_command_args("staging \"update API\" --dry-run");
        assert_eq!(args, vec!["staging", "update API", "--dry-run"]);
    }

    #[test]
    fn format_entries_as_text_includes_user_and_assistant() {
        let path = vec![
            user_entry(EntryId::new(), "hello"),
            assistant_entry(EntryId::new(), "world", None),
        ];
        let text = format_entries_as_text(&path);
        assert!(text.contains("[User]"));
        assert!(text.contains("hello"));
        assert!(text.contains("[Assistant]"));
        assert!(text.contains("world"));
    }
}
