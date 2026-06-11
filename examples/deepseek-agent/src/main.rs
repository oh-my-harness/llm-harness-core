//! Minimal DeepSeek-backed `Agent` example.
//!
//! Run with:
//!
//! ```powershell
//! $env:DEEPSEEK_API_KEY="sk-..."
//! cargo run -p deepseek-agent-example
//! ```
//!
//! Optional environment variables:
//!
//! ```powershell
//! $env:DEEPSEEK_MODEL="deepseek-v4-flash"
//! $env:LLM_HARNESS_PROMPT="Say hello from llm-harness."
//! ```

use std::{
    io::{self, Write},
    sync::Arc,
};

use llm_adapter::deepseek;
use llm_harness::prelude::{Agent, AgentEvent, AgentMessage, AgentOptions, ContentBlock};
use llm_harness_loop::LlmClient;
use tokio::task::JoinHandle;

#[derive(Default)]
struct EventCounts {
    text_chunks: u64,
    thinking_chunks: u64,
    skipped_events: u64,
}

fn assistant_text(messages: &[AgentMessage]) -> String {
    let mut output = String::new();

    for message in messages {
        // The transcript contains user messages, assistant messages, and
        // possibly tool results. This example only prints assistant replies.
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };

        for block in &assistant.content {
            // Assistant content is block-based so it can carry text, thinking,
            // tool calls, images, and future content kinds. Here we only want
            // user-visible text.
            if let ContentBlock::Text { text } = block {
                output.push_str(text);
            }
        }
    }

    output
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    //基础设置：API key、模型名称、初始提示语
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .map_err(|_| anyhow::anyhow!("set DEEPSEEK_API_KEY before running this example"))?;
    let model = std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".into());
    let first_prompt = std::env::var("LLM_HARNESS_PROMPT")
        .ok()
        .filter(|prompt| !prompt.trim().is_empty());
    //构建Deepseek provider的客户端实例
    let client = Arc::new(deepseek::client(api_key)) as Arc<dyn LlmClient>;

    //构建Agent实例，传入客户端和选项
    let mut opts = AgentOptions::new(model.clone());
    opts.system_prompt = Some("You are a concise assistant.".into());
    let agent = Agent::new(client, opts);

    println!("model: {model}");
    println!("type a message and press Enter; use `exit` or `quit` to stop");
    println!();

    if let Some(prompt) = first_prompt {
        println!("You > {prompt}");
        run_turn(&agent, &prompt).await?;
    }

    let stdin = io::stdin();
    let mut input = String::new();
    //输入输出循环，直到用户输入exit或quit
    loop {
        print!("You > ");
        io::stdout().flush()?;

        input.clear();
        if stdin.read_line(&mut input)? == 0 {
            break;
        }

        let prompt = input.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt.eq_ignore_ascii_case("exit") || prompt.eq_ignore_ascii_case("quit") {
            break;
        }

        run_turn(&agent, prompt).await?;
    }

    Ok(())
}

fn spawn_event_counter(agent: &Agent) -> JoinHandle<EventCounts> {
    // Subscribe for this turn only. That avoids mixing stale events from a
    // previous prompt with the events emitted by the current prompt.
    let mut events = agent.subscribe();

    tokio::spawn(async move {
        let mut counts = EventCounts::default();

        loop {
            match events.recv().await {
                Ok(event) => match event.as_ref() {
                    AgentEvent::TextDelta { .. } => counts.text_chunks += 1,
                    AgentEvent::ThinkingDelta { .. } => counts.thinking_chunks += 1,
                    AgentEvent::Error(err) => eprintln!("agent error: {err}"),
                    AgentEvent::AgentEnd { .. } => break,
                    _ => {}
                },
                // A broadcast receiver can lag if the sender produces events
                // faster than this task can consume them. Keep going so later
                // turns do not get stuck behind a lagged receiver.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    counts.skipped_events += skipped;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }

        counts
    })
}

async fn run_turn(agent: &Agent, prompt: &str) -> anyhow::Result<()> {
    // Remember where the transcript was before this turn so we can print only
    // the assistant messages produced for the current user input.
    let message_start = agent.state().messages.len();
    let event_counter = spawn_event_counter(agent);

    // prompt() appends the user message to the existing transcript, runs the
    // full agent loop, and returns after the model stops.
    if let Err(err) = agent.prompt(prompt.to_owned()).await {
        event_counter.abort();
        return Err(err.into());
    }
    let counts = event_counter.await?;

    // Agent::state() exposes the in-memory transcript accumulated so far.
    let state = agent.state();
    let answer = assistant_text(&state.messages[message_start..]);

    println!("Assistant > {answer}");
    println!("------------------------------------------------------------------------------");
    println!(
        "events: {} text chunks, {} thinking chunks",
        counts.text_chunks, counts.thinking_chunks
    );
    if counts.skipped_events > 0 {
        println!(
            "warning: skipped {} broadcast events",
            counts.skipped_events
        );
    }
    println!();

    Ok(())
}
