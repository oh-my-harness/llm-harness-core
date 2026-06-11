//! Minimal DeepSeek-backed `Agent` example.
//!
//! Run with:
//!
//! ```powershell
//! $env:DEEPSEEK_API_KEY="sk-..."
//! cargo run -p llm-harness --example deepseek_agent
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

    //事件订阅
    let mut events = agent.subscribe();

    println!("model: {model}");
    println!("type a message and press Enter; use `exit` or `quit` to stop");
    println!();

    if let Some(prompt) = first_prompt {
        println!("You > {prompt}");
        run_turn(&agent, &mut events, &prompt).await?;
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
        
        run_turn(&agent, &mut events, prompt).await?;
    }

    Ok(())
}

async fn run_turn(
    agent: &Agent,
    events: &mut tokio::sync::broadcast::Receiver<Arc<AgentEvent>>,
    prompt: &str,
) -> anyhow::Result<()> {
    // Remember where the transcript was before this turn so we can print only
    // the assistant messages produced for the current user input.
    let message_start = agent.state().messages.len();

    // prompt() appends the user message to the existing transcript, runs the
    // full agent loop, and returns after the model stops.
    agent.prompt(prompt.to_owned()).await?;

    // Drain events emitted during this turn. For a small CLI example we only
    // count text/thinking chunks; applications can handle every event in real time.
    let mut text_chunks = 0usize;
    let mut thinking_chunks = 0usize;
    while let Ok(event) = events.try_recv() {
        match event.as_ref() {
            AgentEvent::TextDelta { .. } => text_chunks += 1,
            AgentEvent::ThinkingDelta { .. } => thinking_chunks += 1,
            AgentEvent::Error(err) => eprintln!("agent error: {err}"),
            _ => {}
        }
    }

    // Agent::state() exposes the in-memory transcript accumulated so far.
    let state = agent.state();
    let answer = assistant_text(&state.messages[message_start..]);

    println!("Assistant > {answer}");
    println!("------------------------------------------------------------------------------");
    println!("events: {text_chunks} text chunks, {thinking_chunks} thinking chunks");
    println!();

    Ok(())
}
