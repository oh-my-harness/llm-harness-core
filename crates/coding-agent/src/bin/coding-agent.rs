//! CLI for the coding agent.
//!
//! Usage:
//!   coding-agent -p "your prompt"            # one-shot mode
//!   coding-agent --interactive               # REPL / interactive mode
//!   coding-agent --session-id <id> -p "..."  # resume session (one-shot)
//!   coding-agent --list-sessions             # list persisted sessions
//!   echo "prompt" | coding-agent             # pipe a prompt
//!
//! Env:
//!   ANTHROPIC_API_KEY  – required Anthropic API key
//!   CODING_AGENT_MODEL – override model (else reads from settings.json)

use std::io::Read;
use std::sync::Arc;

use llm_adapter::anthropic::AnthropicProvider;
use llm_harness::AgentHarnessEvent;
use llm_harness::session::SessionMetadata;
use llm_harness_loop::RetryConfig;
use llm_harness_types::{AgentEvent, HarnessPhase, ThinkingLevel};

use coding_agent::agent::CodingAgent;
use coding_agent::settings::{SettingsManager, default_config_dir};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

#[tokio::main]
async fn main() {
    let code = run().await;
    std::process::exit(code);
}

async fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();

    // ── Resolve common settings early (needed by both list and prompt paths) ──

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let settings_mgr = SettingsManager::load(&default_config_dir(), Some(&cwd));

    let session_dir = std::env::var("CODING_AGENT_SESSION_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            settings_mgr
                .settings()
                .session_dir
                .as_deref()
                .map(std::path::PathBuf::from)
        })
        .unwrap_or_else(|| {
            dirs_next::data_dir()
                .unwrap_or_else(|| cwd.join(".sessions"))
                .join("coding-agent")
                .join("sessions")
        });

    // ── --list-sessions: enumerate persisted sessions then exit ───────────────

    if args.iter().any(|a| a == "--list-sessions") {
        return list_sessions(&session_dir);
    }

    // ── --delete-session <id>: remove a persisted session then exit ───────────

    if let Some(del_id) = resolve_flag_value(&args, "--delete-session") {
        return delete_session(&session_dir, &del_id);
    }

    // ── Detect mode: interactive vs one-shot ──────────────────────────────────

    let interactive = args.iter().any(|a| a == "--interactive" || a == "-i")
        || (atty::is(atty::Stream::Stdin) && resolve_prompt_opt(&args).is_none());

    let prompt_opt = if interactive {
        None
    } else {
        match resolve_prompt(&args) {
            Ok(p) => Some(p),
            Err(msg) => {
                eprintln!("Error: {msg}");
                return 1;
            }
        }
    };

    // ── Resolve API key and model ──────────────────────────────────────────────

    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("Error: ANTHROPIC_API_KEY environment variable is not set");
            return 1;
        }
    };

    let model = std::env::var("CODING_AGENT_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| settings_mgr.resolved_model(DEFAULT_MODEL));

    // Optional session ID to resume (--session-id <id>).
    let resume_id = resolve_session_id(&args);

    // ── Build and run agent ────────────────────────────────────────────────────

    let client = Arc::new(AnthropicProvider::builder(api_key).build());

    let mut builder = CodingAgent::builder(&model)
        .client(client)
        .session_dir(session_dir);

    if let Some(ref id) = resume_id {
        builder = builder.resume_session(id);
    }

    // Set session name if provided.
    if let Some(name) = resolve_flag_value(&args, "--name") {
        builder = builder.session_name(name);
    }

    // Wire max_tokens from settings (env override not provided — settings win over default).
    if let Some(mt) = settings_mgr.settings().max_tokens {
        builder = builder.max_tokens(mt);
    }

    // Wire thinking_level from settings.
    if let Some(level) = settings_mgr
        .settings()
        .default_thinking_level
        .as_deref()
        .and_then(parse_thinking_level)
    {
        builder = builder.thinking_level(level);
    }

    // Wire auto-compaction: disabled only when settings explicitly set enabled = false.
    if let Some(cs) = settings_mgr.settings().compaction.as_ref() {
        if cs.enabled == Some(false) {
            builder = builder.auto_compact(false);
        }
        if let Some(rt) = cs.reserve_tokens {
            builder = builder.compaction_reserve_tokens(rt);
        }
        if let Some(kr) = cs.keep_recent_tokens {
            builder = builder.compaction_keep_recent_tokens(kr);
        }
    }

    // Wire retry from settings.
    if let Some(ref rs) = settings_mgr.settings().retry {
        if rs.enabled == Some(false) {
            builder = builder.retry(None);
        } else {
            let max_retries = rs.max_retries.unwrap_or(3);
            let base_delay_ms = rs.base_delay_ms.unwrap_or(2_000);
            builder = builder.retry(Some(RetryConfig::new(max_retries, base_delay_ms)));
        }
    }

    let agent = match builder.build().await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error building agent: {e}");
            return 1;
        }
    };

    // Print session ID to stderr so callers can capture it for future --session-id use.
    if let Some(sid) = agent.session_id() {
        eprintln!("session-id: {sid}");
    }

    if interactive {
        return run_interactive(&agent).await;
    }

    let prompt = prompt_opt.unwrap();
    if let Err(e) = stream_prompt(&agent, &prompt).await {
        eprintln!("Error: {e}");
        return 1;
    }

    0
}

// ── Subcommand: --list-sessions ───────────────────────────────────────────────

fn list_sessions(dir: &std::path::Path) -> i32 {
    if !dir.exists() {
        println!(
            "No sessions found (directory does not exist: {})",
            dir.display()
        );
        return 0;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error reading session directory: {e}");
            return 1;
        }
    };

    let mut sessions: Vec<SessionMetadata> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let meta_path = e.path().join("meta.json");
            let bytes = std::fs::read(meta_path).ok()?;
            serde_json::from_slice(&bytes).ok()
        })
        .collect();

    if sessions.is_empty() {
        println!("No sessions found.");
        return 0;
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

    println!("{:<38}  {:<16}  NAME", "SESSION ID", "UPDATED");
    println!("{}", "-".repeat(72));
    for s in &sessions {
        let name = s.name.as_deref().unwrap_or("(unnamed)");
        let updated = s.updated_at.format("%Y-%m-%d %H:%M");
        println!("{:<38}  {:<16}  {}", s.id, updated, name);
    }

    0
}

// ── Arg helpers ───────────────────────────────────────────────────────────────

fn delete_session(dir: &std::path::Path, id: &str) -> i32 {
    let session_path = dir.join(id);
    if !session_path.exists() {
        eprintln!("Session not found: {id}");
        return 1;
    }
    match std::fs::remove_dir_all(&session_path) {
        Ok(()) => {
            println!("Deleted session: {id}");
            0
        }
        Err(e) => {
            eprintln!("Error deleting session {id}: {e}");
            1
        }
    }
}

fn resolve_prompt(args: &[String]) -> Result<String, String> {
    resolve_prompt_opt(args).ok_or_else(|| {
        "No prompt provided. Use -p \"prompt\", pass text as argument, or pipe via stdin.".into()
    })
}

/// Like `resolve_prompt` but returns `None` instead of an error when no prompt is found.
fn resolve_prompt_opt(args: &[String]) -> Option<String> {
    // -p "prompt" or --print "prompt"
    for i in 0..args.len() {
        if (args[i] == "-p" || args[i] == "--print") && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }

    // Positional arg: skip values consumed by flags that take a parameter
    let mut skip_next = false;
    for arg in args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--session-id" || arg == "--name" || arg == "--delete-session" {
            skip_next = true;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }

    // Stdin (non-TTY only)
    if !atty::is(atty::Stream::Stdin) {
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_ok() {
            let trimmed = buf.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }

    None
}

fn resolve_session_id(args: &[String]) -> Option<String> {
    resolve_flag_value(args, "--session-id")
}

fn resolve_flag_value(args: &[String], flag: &str) -> Option<String> {
    for i in 0..args.len() {
        if args[i] == flag && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
    }
    None
}

fn parse_thinking_level(s: &str) -> Option<ThinkingLevel> {
    match s.to_lowercase().as_str() {
        "off" | "none" => Some(ThinkingLevel::Off),
        "low" => Some(ThinkingLevel::Low),
        "medium" | "med" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        _ => None,
    }
}

// ── Streaming helpers ─────────────────────────────────────────────────────────

/// Send a prompt and stream text deltas to stdout; prints a trailing newline.
async fn stream_prompt(
    agent: &coding_agent::agent::CodingAgent,
    prompt: &str,
) -> Result<(), llm_harness_types::HarnessError> {
    use std::io::Write;
    use tokio::sync::broadcast::error::RecvError;

    let mut events = agent.harness().subscribe();
    let printer = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ev) => match ev.as_ref() {
                    AgentHarnessEvent::Agent(AgentEvent::TextDelta { text, .. }) => {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                    }
                    AgentHarnessEvent::PhaseChange {
                        to: HarnessPhase::Idle,
                        ..
                    } => break,
                    _ => {}
                },
                Err(RecvError::Closed) => break,
                Err(RecvError::Lagged(_)) => continue,
            }
        }
    });

    let result = agent.prompt(prompt).await;
    let _ = printer.await;
    println!();
    result
}

// ── Interactive REPL ──────────────────────────────────────────────────────────

async fn run_interactive(agent: &coding_agent::agent::CodingAgent) -> i32 {
    use rustyline::DefaultEditor;
    use rustyline::error::ReadlineError;

    let mut editor = match DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize line editor: {e}");
            return 1;
        }
    };

    loop {
        let line = match editor.readline("> ") {
            Ok(l) => l,
            Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => break,
            Err(e) => {
                eprintln!("Error reading input: {e}");
                return 1;
            }
        };

        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line == "exit" || line == "quit" {
            break;
        }
        let _ = editor.add_history_entry(&line);

        if let Err(e) = stream_prompt(agent, &line).await {
            eprintln!("\nError: {e}");
            return 1;
        }
    }

    0
}
