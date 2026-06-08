use futures::future::BoxFuture;
use llm_harness_types::*;
use serde_json::Value;

/// Find files matching a glob pattern.
pub struct FindTool {
    schema: Value,
}

impl FindTool {
    pub fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match (e.g. '**/*.rs', 'src/*.ts')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: working directory)"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }
}

impl Default for FindTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for FindTool {
    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Results sorted by modification time."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move { run_find(args, ctx).await })
    }
}

async fn run_find(args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
    let pattern = args["pattern"]
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments("pattern required".into()))?;
    let search_path = args["path"].as_str().unwrap_or(".");

    // Use rg --files with --glob to find matching files (respects .gitignore).
    let cmd = format!(
        "rg --files --color=never --glob={} {} 2>/dev/null | sort | head -200",
        shell_escape(pattern),
        shell_escape(search_path),
    );

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

    let text = if output.stdout.is_empty() {
        "No files found.".to_string()
    } else {
        output.stdout.trim_end().to_string()
    };

    Ok(ToolResult {
        content: vec![ContentBlock::Text { text }],
        details: Value::Null,
        terminate: false,
    })
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

    #[tokio::test]
    async fn find_locates_matching_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("main.rs"), "").unwrap();
        std::fs::write(dir.path().join("lib.rs"), "").unwrap();
        std::fs::write(dir.path().join("readme.md"), "").unwrap();

        let env = Arc::new(OsEnv::new(dir.path().to_path_buf()));
        let ctx = make_ctx(env);

        let tool = FindTool::new();
        let args = serde_json::json!({
            "pattern": "*.rs",
            "path": dir.path().to_str().unwrap()
        });
        let result = tool.execute(args, &ctx).await.unwrap();

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(
            text.contains(".rs"),
            "should list .rs files; got: {:?}",
            text
        );
        assert!(
            !text.contains(".md"),
            "should not list .md files; got: {:?}",
            text
        );
    }

    #[tokio::test]
    async fn find_returns_no_files_message() {
        let dir = TempDir::new().unwrap();

        let env = Arc::new(OsEnv::new(dir.path().to_path_buf()));
        let ctx = make_ctx(env);

        let tool = FindTool::new();
        let args = serde_json::json!({
            "pattern": "*.xyz_nonexistent",
            "path": dir.path().to_str().unwrap()
        });
        let result = tool.execute(args, &ctx).await.unwrap();

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("No files"), "got: {:?}", text);
    }
}
