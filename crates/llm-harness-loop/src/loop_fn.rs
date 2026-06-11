use std::sync::Arc;

use futures::{Stream, StreamExt};
use llm_adapter::provider::Provider;
use llm_harness_types::*;

use crate::config::LoopConfig;

fn thinking_budget(level: ThinkingLevel) -> Option<u32> {
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(512),
        ThinkingLevel::Low => Some(1_024),
        ThinkingLevel::Medium => Some(8_192),
        ThinkingLevel::High => Some(32_000),
        ThinkingLevel::XHigh => Some(64_000),
    }
}

/// Start an agent loop from a new context.
pub fn agent_loop(
    client: Arc<dyn Provider>,
    ctx: AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send {
    run_loop(client, ctx, config, false)
}

/// Continue an agent loop without injecting new initial messages.
///
/// Drains stale steer messages (already queued before this call) before the first LLM call.
pub fn agent_loop_continue(
    client: Arc<dyn Provider>,
    ctx: AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send {
    run_loop(client, ctx, config, true)
}

fn run_loop(
    client: Arc<dyn Provider>,
    mut ctx: AgentContext,
    mut config: LoopConfig,
    drain_steer_first: bool,
) -> impl Stream<Item = AgentEvent> + Send {
    async_stream::stream! {
        use crate::{
            dispatch::execute_tool_batch,
            stream_state::StreamingState,
            type_bridge::tools_to_defs,
        };
        use llm_adapter::types::{ChatRequest, Message as AMsg, ToolChoice};
        use tokio::sync::mpsc;

        yield AgentEvent::AgentStart { initial_messages: vec![] };

        // Drain stale steer messages for agent_loop_continue
        if drain_steer_first && let Some(ref mut rx) = config.steer_rx {
            while rx.try_recv().is_ok() {}
        }

        let mut turn_index: u32 = 0;
        let mut new_messages: Vec<AgentMessage> = vec![];

        'main: loop {
            // Check abort
            if config.abort.is_cancelled() {
                yield AgentEvent::Error(AgentError::Aborted);
                break 'main;
            }

            // Drain steer messages between turns
            if turn_index > 0 && let Some(ref mut rx) = config.steer_rx {
                while let Ok(msg) = rx.try_recv() {
                    ctx.messages.push(msg.clone());
                    new_messages.push(msg);
                }
            }

            // TransformContextHook
            if let Some(h) = &config.transform_context {
                match h.transform(ctx).await {
                    Ok(new_ctx) => ctx = new_ctx,
                    Err(e) => {
                        yield AgentEvent::Error(e);
                        break 'main;
                    }
                }
            }

            yield AgentEvent::TurnStart { index: turn_index };

            // Build message list (with optional system message prepended)
            let llm_messages = if let Some(sp) = &ctx.system_prompt {
                let mut v = vec![AMsg::System(sp.clone())];
                match config.convert_to_llm.convert(&ctx.messages).await {
                    Ok(m) => { v.extend(m); v }
                    Err(e) => { yield AgentEvent::Error(e); break 'main; }
                }
            } else {
                match config.convert_to_llm.convert(&ctx.messages).await {
                    Ok(m) => m,
                    Err(e) => { yield AgentEvent::Error(e); break 'main; }
                }
            };

            let tool_defs = tools_to_defs(&config.tools);
            let tool_choice =
                if tool_defs.is_empty() { ToolChoice::None } else { ToolChoice::Auto };

            let mut req_b = ChatRequest::builder(&config.model, config.max_tokens)
                .messages(llm_messages)
                .tools(tool_defs)
                .tool_choice(tool_choice);
            if let Some(t) = config.temperature {
                req_b = req_b.temperature(t);
            }
            if let Some(budget) = thinking_budget(config.thinking_level) {
                req_b = req_b.extended_thinking_budget(budget);
            }
            let req = req_b.build();

            // BeforeProviderRequestHook: allow callers to modify stream options.
            let mut stream_opts = config.stream_options.clone();
            if let Some(h) = &config.before_provider_request {
                h.before_request(&mut stream_opts).await;
            }
            let _ = stream_opts; // currently not forwarded to adapter; reserved for future use

            // Call LLM (with optional retry on transient errors)
            let mut handle = {
                use crate::config::{is_retryable, RetryConfig};
                let retry = config.retry.as_ref().cloned().unwrap_or(RetryConfig {
                    max_retries: 0,
                    base_delay_ms: 0,
                });
                let mut attempt = 0u32;
                loop {
                    match client.chat_stream(&req).await {
                        Ok(h) => break h,
                        Err(e) if is_retryable(&e) && retry.can_retry(attempt) => {
                            let delay = retry.delay_for(attempt, &e);
                            tokio::time::sleep(delay).await;
                            attempt += 1;
                        }
                        Err(e) => {
                            yield AgentEvent::Error(AgentError::Provider(e.to_string()));
                            break 'main;
                        }
                    }
                }
            };

            // Emit MessageStart
            let mut state = StreamingState::new(handle.model().to_owned());
            let message_id = state.message_id.clone();
            yield AgentEvent::MessageStart { message_id: message_id.clone() };

            // Process stream events
            let mut final_message: Option<AssistantMessage> = None;
            loop {
                match handle.events().next().await {
                    Some(Ok(adapter_event)) => {
                        for agent_event in state.process(adapter_event) {
                            if let AgentEvent::MessageEnd { ref message, .. } = agent_event {
                                final_message = Some(message.clone());
                            }
                            yield agent_event;
                        }
                    }
                    Some(Err(e)) => {
                        yield AgentEvent::Error(AgentError::Provider(e.to_string()));
                        break 'main;
                    }
                    None => break,
                }
            }

            let assistant_msg = match final_message {
                Some(m) => m,
                None => {
                    yield AgentEvent::Error(AgentError::Internal(
                        "stream ended without MessageEnd".into(),
                    ));
                    break 'main;
                }
            };

            // AfterProviderResponseHook: observation hook for quota tracking, cost monitoring.
            if let Some(h) = &config.after_provider_response {
                let info = ProviderResponseInfo {
                    status_code: None,
                    response_headers: vec![],
                    usage: assistant_msg.usage.clone(),
                    latency_ms: 0,
                };
                h.after_response(&info).await;
            }

            let assistant_agent_msg = AgentMessage::Assistant(assistant_msg.clone());
            ctx.messages.push(assistant_agent_msg.clone());
            new_messages.push(assistant_agent_msg);

            let stop_reason = assistant_msg.stop_reason.unwrap_or(StopReason::EndTurn);

            if stop_reason == StopReason::ToolUse {
                // Collect tool-use blocks from the assistant message
                let tool_calls: Vec<(String, String, serde_json::Value)> = assistant_msg
                    .content
                    .iter()
                    .filter_map(|cb| {
                        if let ContentBlock::ToolUse { id, name, input } = cb {
                            Some((id.clone(), name.clone(), input.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                // Match tool calls to registered tools
                let calls: Vec<(String, serde_json::Value, Arc<dyn Tool>)> = tool_calls
                    .iter()
                    .filter_map(|(id, name, args)| {
                        config
                            .tools
                            .iter()
                            .find(|t| t.name() == name)
                            .map(|t| (id.clone(), args.clone(), t.clone()))
                    })
                    .collect();

                // Identify unregistered tool calls before moving `calls` into the executor.
                // Leaving orphan ToolUse blocks without a matching ToolResult would cause
                // a 400 error on the next provider request.
                let unmatched: Vec<(String, String)> = tool_calls
                    .iter()
                    .filter(|(id, _, _)| !calls.iter().any(|(cid, _, _)| cid == id))
                    .map(|(id, name, _)| (id.clone(), name.clone()))
                    .collect();

                // Emit ToolExecutionStart for each call
                for (id, name, args) in &tool_calls {
                    yield AgentEvent::ToolExecutionStart {
                        tool_use_id: id.clone(),
                        tool_name: name.clone(),
                        args: args.clone(),
                    };
                }

                let assistant_arc = Arc::new(assistant_msg.clone());
                let env = config.env.clone();
                let abort = config.abort.clone();
                let turn_idx = turn_index;

                // Create a shared update channel whose receiver outlives the batch,
                // so tool send calls don't fail with a disconnected-channel error.
                let (update_tx, _update_rx) = mpsc::channel::<ToolResult>(64);

                let mut results = execute_tool_batch(
                    calls,
                    {
                        let env = env.clone();
                        let abort = abort.clone();
                        let assistant_arc = assistant_arc.clone();
                        let update_tx = update_tx.clone();
                        move |tool_use_id| {
                            ToolContext {
                                env: env.clone(),
                                abort: abort.clone(),
                                tool_use_id,
                                turn_index: turn_idx,
                                assistant_message: assistant_arc.clone(),
                                update_tx: update_tx.clone(),
                            }
                        }
                    },
                    config.default_execution_mode,
                )
                .await;

                // Append synthetic error results for any tool calls not matched to a
                // registered tool; leaving orphan ToolUse blocks causes a provider 400.
                for (id, name) in unmatched {
                    results.push((
                        id,
                        Err(ToolError::Execution(format!("Unknown tool: {name}"))),
                    ));
                }

                // Emit ToolExecutionEnd and build ToolResult messages
                for (id, result) in &results {
                    yield AgentEvent::ToolExecutionEnd {
                        tool_use_id: id.clone(),
                        result: match result {
                            Ok(r) => Ok(r.clone()),
                            Err(e) => Err(ToolError::Execution(e.to_string())),
                        },
                    };
                    let (content, is_error) = match result {
                        Ok(r) => (r.content.clone(), false),
                        Err(e) => (
                            vec![ContentBlock::Text { text: e.to_string() }],
                            true,
                        ),
                    };
                    let tool_result_msg = AgentMessage::ToolResult(ToolResultMessage {
                        tool_use_id: id.clone(),
                        content,
                        is_error,
                        timestamp: chrono::Utc::now(),
                    });
                    ctx.messages.push(tool_result_msg.clone());
                    new_messages.push(tool_result_msg);
                }

                let should_terminate = !results.is_empty()
                    && results.iter().all(|(_, r)| {
                        r.as_ref().map(|tr| tr.terminate).unwrap_or(false)
                    });

                // Call PrepareNextTurnHook before consuming results into TurnEnd
                if !should_terminate && let Some(h) = &config.prepare_next_turn {
                    match h.prepare(PrepareNextTurnCtx {
                        turn_index,
                        last_message: &assistant_msg,
                        last_tool_results: &results,
                    }).await {
                        Ok(directive) => {
                            if let Some(new_ctx) = directive.context { ctx = new_ctx; }
                            if let Some(m) = directive.model { config.model = m; }
                            if let Some(level) = directive.thinking_level { config.thinking_level = level; }
                            if let Some(tools) = directive.tools { config.tools = tools; }
                            if let Some(active) = directive.active_tools {
                                config.tools.retain(|t| active.contains(t.name()));
                            }
                        }
                        Err(e) => {
                            yield AgentEvent::Error(e);
                            yield AgentEvent::TurnEnd {
                                index: turn_index,
                                message: assistant_msg,
                                tool_results: results,
                            };
                            break 'main;
                        }
                    }
                }

                yield AgentEvent::TurnEnd {
                    index: turn_index,
                    message: assistant_msg,
                    tool_results: results,
                };

                if should_terminate {
                    break 'main;
                }

                turn_index += 1;
                continue 'main;
            }

            // Non-tool stop
            yield AgentEvent::TurnEnd {
                index: turn_index,
                message: assistant_msg.clone(),
                tool_results: vec![],
            };

            let should_stop = if let Some(h) = &config.should_stop {
                h.should_stop(ShouldStopCtx {
                    last_assistant: &assistant_msg,
                    stop_reason,
                    turn_index,
                })
                .await
            } else {
                true // default: stop
            };

            if should_stop {
                // Check follow_up channel
                if let Some(ref mut rx) = config.follow_up_rx
                    && let Ok(follow_up_msg) = rx.try_recv()
                {
                    ctx.messages.push(follow_up_msg.clone());
                    new_messages.push(follow_up_msg);
                    turn_index += 1;
                    continue 'main;
                }
                break 'main;
            }

            // should_stop = false → call PrepareNextTurnHook and continue
            if let Some(h) = &config.prepare_next_turn {
                match h.prepare(PrepareNextTurnCtx {
                    turn_index,
                    last_message: &assistant_msg,
                    last_tool_results: &[],
                }).await {
                    Ok(directive) => {
                        if let Some(new_ctx) = directive.context { ctx = new_ctx; }
                        if let Some(m) = directive.model { config.model = m; }
                        if let Some(level) = directive.thinking_level { config.thinking_level = level; }
                        if let Some(tools) = directive.tools { config.tools = tools; }
                        if let Some(active) = directive.active_tools {
                            config.tools.retain(|t| active.contains(t.name()));
                        }
                    }
                    Err(e) => {
                        yield AgentEvent::Error(e);
                        break 'main;
                    }
                }
            }

            turn_index += 1;
        }

        yield AgentEvent::AgentEnd { new_messages };
    }
}

#[cfg(test)]
#[cfg(feature = "test-utils")]
mod tests {
    use crate::test_utils::{MockLlmClient, MockResponse, NoOpEnv};
    use crate::{DefaultConvertToLlm, LoopConfig, agent_loop, agent_loop_continue};
    use futures::StreamExt;
    use futures::future::BoxFuture;
    use llm_adapter::provider::Provider;
    use llm_harness_types::*;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn make_config(responses: Vec<MockResponse>) -> (Arc<MockLlmClient>, LoopConfig) {
        let client = Arc::new(MockLlmClient::new(responses));
        let cfg = LoopConfig {
            model: "test-model".into(),
            max_tokens: 1024,
            temperature: None,
            thinking_level: ThinkingLevel::Off,
            tools: vec![],
            default_execution_mode: ToolExecutionMode::Parallel,
            env: Arc::new(NoOpEnv),
            abort: CancellationToken::new(),
            stream_options: StreamOptions::default(),
            convert_to_llm: Arc::new(DefaultConvertToLlm::new()),
            transform_context: None,
            prepare_next_turn: None,
            should_stop: None,
            before_provider_request: None,
            after_provider_response: None,
            auth: None,
            steer_rx: None,
            follow_up_rx: None,
            retry: None,
        };
        (client, cfg)
    }

    #[tokio::test]
    async fn simple_text_response_emits_correct_events() {
        let (client, cfg) = make_config(vec![MockResponse::text("Hello!")]);
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        let events: Vec<AgentEvent> =
            agent_loop(Arc::clone(&client) as Arc<dyn Provider>, ctx, cfg)
                .collect()
                .await;

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentStart { .. })),
            "missing AgentStart"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::MessageStart { .. })),
            "missing MessageStart"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TextDelta { .. })),
            "missing TextDelta"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::MessageEnd { .. })),
            "missing MessageEnd"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. })),
            "missing TurnEnd"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
            "missing AgentEnd"
        );
        assert_eq!(
            client.call_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn tool_call_then_text_response_two_turns() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct EchoTool {
            called: Arc<AtomicBool>,
        }
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters_schema(&self) -> &serde_json::Value {
                static S: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                S.get_or_init(|| serde_json::json!({}))
            }
            fn execute<'a>(
                &'a self,
                _: serde_json::Value,
                _: &'a ToolContext,
            ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
                self.called.store(true, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async {
                    Ok(ToolResult {
                        content: vec![],
                        details: serde_json::Value::Null,
                        terminate: false,
                    })
                })
            }
        }

        let tool_called = Arc::new(AtomicBool::new(false));
        let tool: Arc<dyn Tool> = Arc::new(EchoTool {
            called: tool_called.clone(),
        });

        let (client_raw, mut cfg) = make_config(vec![
            MockResponse::tool_use("c1", "echo", r#"{"x":1}"#),
            MockResponse::text("Done!"),
        ]);
        cfg.tools = vec![tool];
        let client: Arc<dyn Provider> = Arc::clone(&client_raw) as Arc<dyn Provider>;
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };

        let events: Vec<AgentEvent> = agent_loop(client, ctx, cfg).collect().await;

        assert!(tool_called.load(Ordering::SeqCst), "tool was not executed");
        assert_eq!(
            client_raw
                .call_count
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        let tool_exec_ends = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
            .count();
        assert_eq!(tool_exec_ends, 1);
    }

    #[tokio::test]
    async fn tool_with_terminate_true_stops_loop() {
        struct TerminateTool;
        impl Tool for TerminateTool {
            fn name(&self) -> &str {
                "term"
            }
            fn description(&self) -> &str {
                ""
            }
            fn parameters_schema(&self) -> &serde_json::Value {
                static S: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                S.get_or_init(|| serde_json::json!({}))
            }
            fn execute<'a>(
                &'a self,
                _: serde_json::Value,
                _: &'a ToolContext,
            ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        content: vec![],
                        details: serde_json::Value::Null,
                        terminate: true,
                    })
                })
            }
        }

        let (client_raw, mut cfg) = make_config(vec![MockResponse::tool_use("c1", "term", "{}")]);
        cfg.tools = vec![Arc::new(TerminateTool)];
        let client: Arc<dyn Provider> = Arc::clone(&client_raw) as Arc<dyn Provider>;

        let events: Vec<AgentEvent> = agent_loop(
            client,
            AgentContext {
                system_prompt: None,
                messages: vec![],
            },
            cfg,
        )
        .collect()
        .await;

        assert_eq!(
            client_raw
                .call_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        );
    }

    #[tokio::test]
    async fn abort_signal_stops_loop() {
        let (_, cfg) = make_config(vec![MockResponse::text("hello")]);
        let abort = cfg.abort.clone();
        abort.cancel();

        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        let events: Vec<AgentEvent> = agent_loop(
            Arc::new(MockLlmClient::new(vec![])) as Arc<dyn Provider>,
            ctx,
            cfg,
        )
        .collect()
        .await;

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Error(AgentError::Aborted)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::AgentEnd { .. }))
        );
    }

    #[tokio::test]
    async fn agent_end_carries_new_messages() {
        let (client, cfg) = make_config(vec![MockResponse::text("Hi!")]);
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        let events: Vec<AgentEvent> =
            agent_loop(Arc::clone(&client) as Arc<dyn Provider>, ctx, cfg)
                .collect()
                .await;

        let agent_end = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
            .unwrap();
        if let AgentEvent::AgentEnd { new_messages } = agent_end {
            assert!(!new_messages.is_empty());
            assert!(
                new_messages
                    .iter()
                    .any(|m| matches!(m, AgentMessage::Assistant(_)))
            );
        }
    }

    #[tokio::test]
    async fn agent_loop_continue_drains_stale_steer() {
        use tokio::sync::mpsc;
        let (steer_tx, steer_rx) = mpsc::channel(16);
        let stale_msg = AgentMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "stale".into(),
            }],
            timestamp: chrono::Utc::now(),
        });
        steer_tx.send(stale_msg).await.unwrap();

        let (client, mut cfg) = make_config(vec![MockResponse::text("response")]);
        cfg.steer_rx = Some(steer_rx);

        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        let events: Vec<AgentEvent> =
            agent_loop_continue(Arc::clone(&client) as Arc<dyn Provider>, ctx, cfg)
                .collect()
                .await;

        let agent_end = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentEnd { .. }))
            .unwrap();
        if let AgentEvent::AgentEnd { new_messages } = agent_end {
            let has_stale = new_messages.iter().any(|m| {
                if let AgentMessage::User(u) = m {
                    u.content
                        .iter()
                        .any(|c| matches!(c, ContentBlock::Text { text } if text == "stale"))
                } else {
                    false
                }
            });
            assert!(!has_stale, "stale steer message should have been discarded");
        }
    }

    #[tokio::test]
    async fn agent_loop_continue_agent_start_has_empty_initial_messages() {
        let (client, cfg) = make_config(vec![MockResponse::text("hi")]);
        let ctx = AgentContext {
            system_prompt: None,
            messages: vec![],
        };
        let events: Vec<AgentEvent> =
            agent_loop_continue(Arc::clone(&client) as Arc<dyn Provider>, ctx, cfg)
                .collect()
                .await;

        let agent_start = events
            .iter()
            .find(|e| matches!(e, AgentEvent::AgentStart { .. }))
            .unwrap();
        if let AgentEvent::AgentStart { initial_messages } = agent_start {
            assert!(initial_messages.is_empty());
        }
    }
}
