use futures::future::BoxFuture;
use llm_harness_types::*;
use serde_json::Value;

/// List directory contents.
pub struct LsTool {
    schema: Value,
}

impl LsTool {
    pub fn new() -> Self {
        Self {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to list (default: working directory)"
                    }
                },
                "required": []
            }),
        }
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for LsTool {
    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "List the contents of a directory."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute<'a>(
        &'a self,
        args: Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move { run_ls(args, ctx).await })
    }
}

async fn run_ls(args: Value, ctx: &ToolContext) -> Result<ToolResult, ToolError> {
    let path_str = args["path"].as_str().unwrap_or(".");
    let abs_path = resolve_path(path_str, ctx.env.working_dir());

    let entries = ctx
        .env
        .list_dir(&abs_path, ctx.abort.clone())
        .await
        .map_err(|e| ToolError::Execution(format!("Could not list {}: {}", path_str, e)))?;

    let text = format_entries(&entries, path_str);
    Ok(ToolResult {
        content: vec![ContentBlock::Text { text }],
        details: Value::Null,
        terminate: false,
    })
}

fn format_entries(entries: &[FileInfo], path: &str) -> String {
    if entries.is_empty() {
        return format!("{} (empty)", path);
    }

    let mut lines: Vec<String> = entries
        .iter()
        .map(|e| {
            let name = e
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if e.is_dir {
                format!("{}/", name)
            } else {
                format!("{} ({})", name, format_size(e.size))
            }
        })
        .collect();

    lines.sort();
    lines.join("\n")
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn resolve_path(path: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
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
    fn format_entries_empty() {
        let text = format_entries(&[], ".");
        assert!(text.contains("empty"), "got: {}", text);
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(2048), "2.0KB");
    }

    #[tokio::test]
    async fn ls_lists_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "x").unwrap();
        std::fs::write(dir.path().join("beta.rs"), "y").unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let env = Arc::new(OsEnv::new(dir.path().to_path_buf()));
        let ctx = make_ctx(env);

        let tool = LsTool::new();
        let args = serde_json::json!({ "path": dir.path().to_str().unwrap() });
        let result = tool.execute(args, &ctx).await.unwrap();

        let text = match &result.content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("alpha.txt"), "got: {}", text);
        assert!(text.contains("beta.rs"), "got: {}", text);
        assert!(text.contains("subdir/"), "got: {}", text);
    }
}
