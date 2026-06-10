use futures::future::BoxFuture;
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use llm_harness_types::*;
use regex::RegexBuilder;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Search for a regex pattern in files.
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
        "Search for a regex pattern in files. Respects .gitignore. Use include to filter by file type."
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

    let root = resolve_path(search_path, ctx.env.working_dir());
    if !root.exists() {
        return Err(ToolError::Execution(format!(
            "Path not found: {}",
            root.display()
        )));
    }

    let regex = RegexBuilder::new(pattern)
        .build()
        .map_err(|e| ToolError::InvalidArguments(format!("invalid regex pattern: {e}")))?;
    let include_matcher = include.map(build_glob_matcher).transpose()?;
    let files = collect_search_files(&root, include_matcher.as_ref(), ctx)?;

    let mut output_lines = Vec::new();
    for file in files {
        if ctx.abort.is_cancelled() {
            return Err(ToolError::Aborted);
        }

        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let lines: Vec<&str> = text.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !regex.is_match(line) {
                continue;
            }

            let line_no = idx + 1;
            let start = line_no.saturating_sub(context_lines as usize).max(1);
            let end = (line_no + context_lines as usize).min(lines.len());
            let display_path = display_path(&file, &root);

            for current in start..=end {
                let sep = if current == line_no { ":" } else { "-" };
                output_lines.push(format!(
                    "{}{}{}{} {}",
                    display_path,
                    sep,
                    current,
                    sep,
                    lines[current - 1]
                ));
            }
        }
    }

    let text = if output_lines.is_empty() {
        "No matches found.".to_string()
    } else {
        output_lines.join("\n")
    };

    Ok(ToolResult {
        content: vec![ContentBlock::Text { text }],
        details: Value::Null,
        terminate: false,
    })
}

fn collect_search_files(
    root: &Path,
    include: Option<&GlobMatcher>,
    ctx: &ToolContext,
) -> Result<Vec<PathBuf>, ToolError> {
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }
    if !root.is_dir() {
        return Err(ToolError::Execution(format!(
            "Not a file or directory: {}",
            root.display()
        )));
    }

    let mut files = Vec::new();
    for entry in WalkBuilder::new(root).build() {
        if ctx.abort.is_cancelled() {
            return Err(ToolError::Aborted);
        }

        let entry = entry.map_err(|e| ToolError::Execution(e.to_string()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(path);
        if include.is_none_or(|matcher| matches_glob(matcher, relative)) {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn build_glob_matcher(pattern: &str) -> Result<GlobMatcher, ToolError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|e| ToolError::InvalidArguments(format!("invalid include glob: {e}")))
}

fn matches_glob(matcher: &GlobMatcher, relative: &Path) -> bool {
    let relative_posix = to_posix_path(relative);
    if matcher.is_match(&relative_posix) {
        return true;
    }

    relative
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| matcher.is_match(name))
}

fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn display_path(path: &Path, root: &Path) -> String {
    if root.is_dir() {
        let relative = path.strip_prefix(root).unwrap_or(path);
        to_posix_path(relative)
    } else {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }
}

fn to_posix_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

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
    fn include_glob_matches_basename() {
        let matcher = build_glob_matcher("*.rs").unwrap();
        assert!(matches_glob(&matcher, Path::new("src/main.rs")));
        assert!(!matches_glob(&matcher, Path::new("README.md")));
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
