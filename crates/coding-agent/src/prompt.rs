use llm_harness::Skill;
use llm_harness_types::Tool;

/// A pre-loaded project context file (e.g. CLAUDE.md).
pub struct ContextFile {
    /// Relative or absolute path shown in the prompt.
    pub path: String,
    /// File contents.
    pub content: String,
}

/// Options for building the system prompt.
pub struct SystemPromptOptions<'a> {
    /// Active tools, in order they should appear in the tool list.
    pub tools: &'a [&'a dyn Tool],
    /// Absolute path to the working directory.
    pub cwd: &'a str,
    /// Pre-loaded project context files.
    pub context_files: &'a [ContextFile],
    /// Pre-loaded skills.
    pub skills: &'a [Skill],
    /// Replace the default prompt body entirely.
    pub custom_prompt: Option<&'a str>,
    /// Text appended after the main prompt body.
    pub append: Option<&'a str>,
    /// Extra guideline bullets added after the built-in ones.
    pub extra_guidelines: &'a [String],
}

/// Build the agent system prompt.
///
/// Returns a fully assembled string ready to pass as the `system` parameter.
pub fn build_system_prompt(opts: &SystemPromptOptions<'_>) -> String {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let cwd = opts.cwd.replace('\\', "/");

    let body = if let Some(custom) = opts.custom_prompt {
        custom.to_string()
    } else {
        build_default_body(opts, &date, &cwd)
    };

    let mut prompt = body;

    if let Some(extra) = opts.append {
        prompt.push_str("\n\n");
        prompt.push_str(extra);
    }

    if !opts.context_files.is_empty() {
        prompt.push_str("\n\n<project_context>\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for cf in opts.context_files {
            prompt.push_str(&format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                cf.path, cf.content
            ));
        }
        prompt.push_str("</project_context>");
    }

    let has_read = opts.tools.iter().any(|t| t.name() == "read");
    if has_read && !opts.skills.is_empty() {
        prompt.push_str(&llm_harness::format_skills_for_system_prompt(opts.skills));
    }

    prompt.push_str(&format!("\nCurrent date: {date}"));
    prompt.push_str(&format!("\nCurrent working directory: {cwd}"));

    prompt
}

fn build_default_body(opts: &SystemPromptOptions<'_>, _date: &str, _cwd: &str) -> String {
    let tool_list = if opts.tools.is_empty() {
        "(none)".to_string()
    } else {
        opts.tools
            .iter()
            .map(|t| format!("- {}: {}", t.name(), t.description()))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let has_bash = opts.tools.iter().any(|t| t.name() == "bash");
    let has_grep = opts.tools.iter().any(|t| t.name() == "grep");
    let has_find = opts.tools.iter().any(|t| t.name() == "find");
    let has_ls = opts.tools.iter().any(|t| t.name() == "ls");

    let mut guidelines: Vec<&str> = vec![];
    let mut guideline_strs: Vec<String> = vec![];

    if has_bash && !has_grep && !has_find && !has_ls {
        guidelines.push("Use bash for file operations like ls, rg, find");
    }

    for g in opts.extra_guidelines {
        let s = g.trim();
        if !s.is_empty() && !guidelines.contains(&s) {
            guideline_strs.push(s.to_string());
        }
    }

    guidelines.push("Be concise in your responses");
    guidelines.push("Show file paths clearly when working with files");

    let mut all_guidelines: Vec<String> = guidelines.iter().map(|s| s.to_string()).collect();
    all_guidelines.extend(guideline_strs);

    let guidelines_text = all_guidelines
        .iter()
        .map(|g| format!("- {g}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are an expert coding assistant. \
         You help users by reading files, executing commands, editing code, and writing new files.\n\n\
         Available tools:\n{tool_list}\n\n\
         Guidelines:\n{guidelines_text}"
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTool {
        name: &'static str,
        desc: &'static str,
    }

    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            self.desc
        }
        fn parameters_schema(&self) -> &serde_json::Value {
            static S: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            S.get_or_init(|| serde_json::json!({}))
        }
        fn execute<'a>(
            &'a self,
            _args: serde_json::Value,
            _ctx: &'a llm_harness_types::ToolContext,
        ) -> futures::future::BoxFuture<
            'a,
            Result<llm_harness_types::ToolResult, llm_harness_types::ToolError>,
        > {
            Box::pin(async { Err(llm_harness_types::ToolError::Aborted) })
        }
    }

    #[test]
    fn prompt_contains_tool_name_and_desc() {
        let read = MockTool {
            name: "read",
            desc: "Read a file",
        };
        let bash = MockTool {
            name: "bash",
            desc: "Run a command",
        };
        let tools: Vec<&dyn Tool> = vec![&read, &bash];
        let opts = SystemPromptOptions {
            tools: &tools,
            cwd: "/home/user/project",
            context_files: &[],
            skills: &[],
            custom_prompt: None,
            append: None,
            extra_guidelines: &[],
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("read: Read a file"), "got: {prompt}");
        assert!(prompt.contains("bash: Run a command"), "got: {prompt}");
        assert!(prompt.contains("/home/user/project"), "got: {prompt}");
    }

    #[test]
    fn prompt_appends_context_files() {
        let tools: Vec<&dyn Tool> = vec![];
        let context_files = vec![ContextFile {
            path: "CLAUDE.md".into(),
            content: "# Rules\nBe careful.".into(),
        }];
        let opts = SystemPromptOptions {
            tools: &tools,
            cwd: "/tmp",
            context_files: &context_files,
            skills: &[],
            custom_prompt: None,
            append: None,
            extra_guidelines: &[],
        };
        let prompt = build_system_prompt(&opts);
        assert!(
            prompt.contains("<project_instructions path=\"CLAUDE.md\">"),
            "got: {prompt}"
        );
        assert!(prompt.contains("Be careful."), "got: {prompt}");
    }

    #[test]
    fn custom_prompt_replaces_default_body() {
        let tools: Vec<&dyn Tool> = vec![];
        let opts = SystemPromptOptions {
            tools: &tools,
            cwd: "/tmp",
            context_files: &[],
            skills: &[],
            custom_prompt: Some("Custom instructions only."),
            append: None,
            extra_guidelines: &[],
        };
        let prompt = build_system_prompt(&opts);
        assert!(
            prompt.starts_with("Custom instructions only."),
            "got: {prompt}"
        );
        assert!(!prompt.contains("expert coding assistant"), "got: {prompt}");
    }

    #[test]
    fn prompt_includes_date_and_cwd() {
        let tools: Vec<&dyn Tool> = vec![];
        let opts = SystemPromptOptions {
            tools: &tools,
            cwd: "/workspace",
            context_files: &[],
            skills: &[],
            custom_prompt: None,
            append: None,
            extra_guidelines: &[],
        };
        let prompt = build_system_prompt(&opts);
        assert!(prompt.contains("Current date:"), "got: {prompt}");
        assert!(
            prompt.contains("Current working directory: /workspace"),
            "got: {prompt}"
        );
    }
}
