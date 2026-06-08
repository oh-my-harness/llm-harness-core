use std::path::{Path, PathBuf};
use std::sync::Arc;

use llm_harness::{AgentHarness, AgentHarnessOptions, OsEnv};
use llm_harness_loop::LlmClient;
use llm_harness_types::{ExecutionEnv, HarnessError, Tool};

use crate::prompt::{ContextFile, SystemPromptOptions, build_system_prompt};
use crate::tools::{ALL_TOOL_NAMES, DEFAULT_TOOL_NAMES, create_all_tools};

/// High-level coding agent that wires built-in tools, system prompt, and session together.
pub struct CodingAgent {
    harness: AgentHarness,
}

impl CodingAgent {
    /// Create a builder for configuring and constructing a `CodingAgent`.
    pub fn builder(model: impl Into<String>) -> CodingAgentBuilder {
        CodingAgentBuilder::new(model)
    }

    /// Send a prompt and wait for the run to complete.
    pub async fn prompt(&self, text: &str) -> Result<(), HarnessError> {
        self.harness.prompt(text).await?;
        self.harness.wait_for_idle().await;
        Ok(())
    }

    /// Access the underlying `AgentHarness` (e.g. for event subscription).
    pub fn harness(&self) -> &AgentHarness {
        &self.harness
    }
}

// ── Builder ────────────────────────────────────────────────────────────────────

/// Builder for `CodingAgent`.
pub struct CodingAgentBuilder {
    model: String,
    env: Option<Arc<dyn ExecutionEnv>>,
    client: Option<Arc<dyn LlmClient>>,
    active_tools: Option<Vec<String>>,
    allowed_tools: Option<Vec<String>>,
    cwd: Option<PathBuf>,
    load_context: bool,
    extra_context_files: Vec<ContextFile>,
    extra_guidelines: Vec<String>,
    custom_prompt: Option<String>,
    append_prompt: Option<String>,
}

impl CodingAgentBuilder {
    fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            env: None,
            client: None,
            active_tools: None,
            allowed_tools: None,
            cwd: None,
            load_context: true,
            extra_context_files: vec![],
            extra_guidelines: vec![],
            custom_prompt: None,
            append_prompt: None,
        }
    }

    /// Set the execution environment (default: `OsEnv::new(cwd)`).
    pub fn env(mut self, env: Arc<dyn ExecutionEnv>) -> Self {
        self.env = Some(env);
        self
    }

    /// Set the LLM client.
    pub fn client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.client = Some(client);
        self
    }

    /// Override the working directory (default: current process directory).
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Set which built-in tools are active (default: read, bash, edit, write).
    pub fn active_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.active_tools = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Restrict to a subset of tool names (allowlist).
    pub fn allowed_tools(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.allowed_tools = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// Disable auto-loading of CLAUDE.md / AGENTS.md from the directory tree.
    pub fn no_context(mut self) -> Self {
        self.load_context = false;
        self
    }

    /// Add a context file to the system prompt.
    pub fn context_file(mut self, path: impl Into<String>, content: impl Into<String>) -> Self {
        self.extra_context_files.push(ContextFile {
            path: path.into(),
            content: content.into(),
        });
        self
    }

    /// Add an extra guideline bullet to the system prompt.
    pub fn guideline(mut self, text: impl Into<String>) -> Self {
        self.extra_guidelines.push(text.into());
        self
    }

    /// Replace the default system prompt body entirely.
    pub fn custom_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.custom_prompt = Some(prompt.into());
        self
    }

    /// Append text after the generated system prompt.
    pub fn append_prompt(mut self, text: impl Into<String>) -> Self {
        self.append_prompt = Some(text.into());
        self
    }

    /// Build the `CodingAgent`.
    pub async fn build(self) -> Result<CodingAgent, BuildError> {
        let cwd = self
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let env: Arc<dyn ExecutionEnv> = self
            .env
            .unwrap_or_else(|| Arc::new(OsEnv::new(cwd.clone())));

        let client = self.client.ok_or(BuildError::MissingClient)?;

        // Determine active tools respecting allowlist.
        let active_names: Vec<&str> = {
            let requested: Vec<String> = self
                .active_tools
                .unwrap_or_else(|| DEFAULT_TOOL_NAMES.iter().map(|s| s.to_string()).collect());
            let allowed = self.allowed_tools.as_deref();
            ALL_TOOL_NAMES
                .iter()
                .copied()
                .filter(|name| {
                    requested.iter().any(|r| r == *name)
                        && allowed.map_or(true, |a| a.iter().any(|x| x == name))
                })
                .collect()
        };

        let all_tools = create_all_tools();
        let active_tools: Vec<Arc<dyn Tool>> = all_tools
            .iter()
            .filter(|t| active_names.contains(&t.name()))
            .cloned()
            .collect();

        // Load project context files.
        let mut context_files = if self.load_context {
            load_project_context_files(&cwd)
        } else {
            vec![]
        };
        context_files.extend(self.extra_context_files);

        // Build system prompt.
        let tool_refs: Vec<&dyn Tool> = active_tools.iter().map(|t| t.as_ref()).collect();
        let system_prompt = build_system_prompt(&SystemPromptOptions {
            tools: &tool_refs,
            cwd: &cwd.to_string_lossy(),
            context_files: &context_files,
            skills: &[],
            custom_prompt: self.custom_prompt.as_deref(),
            append: self.append_prompt.as_deref(),
            extra_guidelines: &self.extra_guidelines,
        });

        let opts = build_opts(self.model, active_tools, system_prompt);

        let harness = AgentHarness::new_in_memory(client, env, opts).await;
        Ok(CodingAgent { harness })
    }
}

fn build_opts(
    model: String,
    tools: Vec<Arc<dyn Tool>>,
    system_prompt: String,
) -> AgentHarnessOptions {
    let mut opts = AgentHarnessOptions::new(model);
    opts.tools = tools;
    opts.system_prompt = Some(system_prompt);
    opts
}

// ── Context file loading ───────────────────────────────────────────────────────

/// Names to look for in each directory (checked in order).
const CONTEXT_FILE_NAMES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Load CLAUDE.md / AGENTS.md from `cwd` and all ancestor directories.
///
/// Files are returned in outermost-first order (root → cwd), so the most
/// specific file overrides general ones when appended in sequence.
pub fn load_project_context_files(cwd: &Path) -> Vec<ContextFile> {
    let mut ancestors: Vec<ContextFile> = vec![];
    let mut current = cwd.to_path_buf();
    let mut seen: std::collections::HashSet<PathBuf> = Default::default();

    loop {
        if let Some(cf) = load_context_from_dir(&current) {
            if seen.insert(cf.path.clone().into()) {
                ancestors.push(cf);
            }
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }

    ancestors.reverse(); // root first
    ancestors
}

fn load_context_from_dir(dir: &Path) -> Option<ContextFile> {
    for name in CONTEXT_FILE_NAMES {
        let path = dir.join(name);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                return Some(ContextFile {
                    path: path.display().to_string(),
                    content,
                });
            }
        }
    }
    None
}

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("LLM client is required; call .client(...)")]
    MissingClient,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_context_files_finds_claude_md() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# Project rules").unwrap();

        let files = load_project_context_files(dir.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Project rules"));
    }

    #[test]
    fn load_context_files_returns_outermost_first() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "outer").unwrap();
        std::fs::write(sub.join("CLAUDE.md"), "inner").unwrap();

        let files = load_project_context_files(&sub);
        // Last element should be the innermost (sub/)
        assert_eq!(files.last().unwrap().content, "inner");
        // Outer comes before inner
        let outer_idx = files.iter().position(|f| f.content == "outer").unwrap();
        let inner_idx = files.iter().position(|f| f.content == "inner").unwrap();
        assert!(outer_idx < inner_idx);
    }

    #[test]
    fn load_context_files_empty_dir() {
        let dir = TempDir::new().unwrap();
        // No CLAUDE.md or AGENTS.md present in dir itself, but walk may find them up the tree.
        // At minimum the function should not panic.
        let _ = load_project_context_files(dir.path());
    }
}
