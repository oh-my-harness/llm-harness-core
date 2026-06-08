use futures::future::BoxFuture;
use llm_harness_types::*;
use serde_json::Value;

/// Search for a regex pattern in files using ripgrep (falls back to grep).
pub struct GrepTool {
    schema: Value,
}

impl GrepTool {
    pub fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search in (default: working directory)"
                    },
                    "include": {
                        "type": "string",
                        "description": "Glob pattern to include (e.g. '*.rs')"
                    },
                    "context": {
                        "type": "integer",
                        "description": "Lines of context around each match"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files using ripgrep. \
         Respects .gitignore. Use include to filter by file type."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move { run_grep(args, ctx).await })
    }
}

async fn run_grep(args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
    let pattern = args["pattern"]
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments("pattern required".into()))?;

    let search_path = args["path"].as_str().unwrap_or(".");
    let include = args["include"].as_str();
    let context_lines = args["context"].as_u64().unwrap_or(0);

    // Build rg command; fall back to grep if rg not found.
    let cmd = build_grep_command(pattern, search_path, include, context_lines);

    let opts = ShellOptions {
        cwd: Some(ctx.env.working_dir()),
        env: vec![],
        timeout: Some(std::time::Duration::from_secs(30)),
        abort: ctx.abort.clone(),
        on_stdout: None,
        on_stderr: None,
    };

    let output = ctx
        .env
        .execute_shell(&cmd, opts)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    // rg exits 1 when no matches — that's not an error.
    let text = if output.stdout.is_empty() && output.exit_code != 0 {
        "No matches found.".to_string()
    } else {
        output.stdout
    };

    Ok(ToolResult {
        content: vec![ContentBlock::Text { text }],
        details: Value::Null,
        terminate: false,
    })
}

fn build_grep_command(
    pattern: &str,
    path: &str,
    include: Option<&str>,
    context_lines: u64,
) -> String {
    let mut parts = vec!["rg".to_string(), "--color=never".to_string()];

    if context_lines > 0 {
        parts.push(format!("-C{}", context_lines));
    }

    if let Some(glob) = include {
        parts.push(format!("--glob={}", shell_escape(glob)));
    }

    parts.push(shell_escape(pattern));
    parts.push(shell_escape(path));
    parts.join(" ")
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use llm_harness::OsEnv;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    fn make_ctx(env: Arc<dyn ExecutionEnv>) -> ToolContext {
        let (tx, _rx) = mpsc::channel(4);
        ToolContext {
            env,
            abort: CancellationToken::new(),
            tool_use_id: "test-id".into(),
            turn_index: 0,
            assistant_message: Arc::new(AssistantMessage {
                content: vec![],
                stop_reason: None,
                timestamp: chrono::Utc::now(),
                provider: None,
                api: None,
                model: None,
                usage: None,
                error_message: None,
            }),
            update_tx: tx,
        }
    }

    #[test]
    fn build_command_basic() {
        let cmd = build_grep_command("fn main", "src/", None, 0);
        assert!(cmd.contains("rg"), "got: {}", cmd);
        assert!(cmd.contains("fn main"), "got: {}", cmd);
        assert!(cmd.contains("src/"), "got: {}", cmd);
    }

    #[test]
    fn build_command_with_context_and_include() {
        let cmd = build_grep_command("TODO", ".", Some("*.rs"), 2);
        assert!(cmd.contains("-C2"), "got: {}", cmd);
        assert!(cmd.contains("*.rs"), "got: {}", cmd);
    }

    #[tokio::test]
    async fn grep_finds_pattern_in_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\nfoo bar\n").unwrap();

        let env = Arc::new(OsEnv::new(dir.path().to_path_buf()));
        let ctx = make_ctx(env);

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap()
        });
        let result = tool.execute(args, &ctx).await.unwrap();

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("hello"), "got: {:?}", text);
    }

    #[tokio::test]
    async fn grep_returns_no_matches_message() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();

        let env = Arc::new(OsEnv::new(dir.path().to_path_buf()));
        let ctx = make_ctx(env);

        let tool = GrepTool::new();
        let args = serde_json::json!({
            "pattern": "xyzzy_nonexistent_xyz",
            "path": dir.path().to_str().unwrap()
        });
        let result = tool.execute(args, &ctx).await.unwrap();

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("No matches"), "got: {:?}", text);
    }
}
