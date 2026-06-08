//! Print-mode CLI: send a prompt, print the final assistant response, exit.
//!
//! Usage:
//!   coding-agent -p "your prompt"
//!   echo "your prompt" | coding-agent
//!
//! Env:
//!   ANTHROPIC_API_KEY  – required Anthropic API key
//!   CODING_AGENT_MODEL – override model (else reads from settings.json)

use std::io::Read;
use std::sync::Arc;

use llm_adapter::anthropic::AnthropicProvider;
use llm_harness_types::{AgentMessage, ContentBlock};

use coding_agent::agent::CodingAgent;
use coding_agent::settings::{SettingsManager, default_config_dir};

const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

#[tokio::main]
async fn main() {
    let code = run().await;
    std::process::exit(code);
}

async fn run() -> i32 {
    // ── Resolve prompt text ────────────────────────────────────────────────────

    let args: Vec<String> = std::env::args().collect();
    let prompt = match resolve_prompt(&args) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("Error: {msg}");
            return 1;
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

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let settings_mgr = SettingsManager::load(&default_config_dir(), Some(&cwd));

    let model = std::env::var("CODING_AGENT_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| settings_mgr.resolved_model(DEFAULT_MODEL));

    // Session dir: from env, settings, or default to ~/.local/share/coding-agent/sessions
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

    // ── Build and run agent ────────────────────────────────────────────────────

    let client = Arc::new(AnthropicProvider::builder(api_key).build());

    let mut builder = CodingAgent::builder(&model)
        .client(client)
        .session_dir(session_dir);

    let agent = match builder.build().await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error building agent: {e}");
            return 1;
        }
    };

    if let Err(e) = agent.prompt(&prompt).await {
        eprintln!("Error: {e}");
        return 1;
    }

    // ── Extract and print the last assistant response ──────────────────────────

    let context = match agent.harness().build_context().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error reading session: {e}");
            return 1;
        }
    };

    let last_text = context.messages.iter().rev().find_map(|m| {
        if let AgentMessage::Assistant(am) = m {
            let text: String = am
                .content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            if !text.is_empty() { Some(text) } else { None }
        } else {
            None
        }
    });

    match last_text {
        Some(text) => {
            println!("{text}");
            0
        }
        None => {
            eprintln!("No response received");
            1
        }
    }
}

fn resolve_prompt(args: &[String]) -> Result<String, String> {
    // -p "prompt" or --print "prompt"
    for i in 0..args.len() {
        if (args[i] == "-p" || args[i] == "--print") && i + 1 < args.len() {
            return Ok(args[i + 1].clone());
        }
    }

    // Positional arg (not a flag)
    for arg in args.iter().skip(1) {
        if !arg.starts_with('-') {
            return Ok(arg.clone());
        }
    }

    // Stdin
    if !atty::is(atty::Stream::Stdin) {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("Failed to read stdin: {e}"))?;
        let trimmed = buf.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    Err("No prompt provided. Use -p \"prompt\", pass text as argument, or pipe via stdin.".into())
}
