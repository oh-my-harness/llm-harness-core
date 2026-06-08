pub mod bash;
pub mod edit;
pub mod find;
pub mod grep;
pub mod ls;
pub mod read;
pub mod write;

use std::sync::Arc;

use llm_harness_types::Tool;

/// All built-in tool names in declaration order.
pub const ALL_TOOL_NAMES: &[&str] = &["read", "bash", "edit", "write", "grep", "find", "ls"];

/// Default active tool names (read/write/edit/bash).
pub const DEFAULT_TOOL_NAMES: &[&str] = &["read", "bash", "edit", "write"];

/// Create one instance of every built-in tool.
pub fn create_all_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(read::ReadTool::new()),
        Arc::new(bash::BashTool::new()),
        Arc::new(edit::EditTool::new()),
        Arc::new(write::WriteTool::new()),
        Arc::new(grep::GrepTool::new()),
        Arc::new(find::FindTool::new()),
        Arc::new(ls::LsTool::new()),
    ]
}

/// Filter a slice of tools to only those whose names appear in `names`.
/// Output order matches the order of `names`.
pub fn select_tools<'a>(all: &'a [Arc<dyn Tool>], names: &[&str]) -> Vec<&'a Arc<dyn Tool>> {
    names
        .iter()
        .filter_map(|name| all.iter().find(|t| t.name() == *name))
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_all_tools_returns_seven() {
        let tools = create_all_tools();
        assert_eq!(tools.len(), ALL_TOOL_NAMES.len());
    }

    #[test]
    fn create_all_tools_names_match_constants() {
        let tools = create_all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        for &expected in ALL_TOOL_NAMES {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }
    }

    #[test]
    fn select_tools_preserves_order() {
        let all = create_all_tools();
        let selected = select_tools(&all, &["write", "read"]);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].name(), "write");
        assert_eq!(selected[1].name(), "read");
    }

    #[test]
    fn select_tools_skips_unknown_names() {
        let all = create_all_tools();
        let selected = select_tools(&all, &["read", "nonexistent"]);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name(), "read");
    }
}
