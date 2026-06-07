use std::sync::Arc;

use llm_harness_types::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::convert::ConvertToLlmHook;

/// Agent loop 的配置。`agent_loop` 会消费此结构（所有权转移）。
pub struct LoopConfig {
    // === LLM sampling ===
    /// 模型 ID（例如: `"claude-sonnet-4-6"`）。
    pub model: String,
    /// 最大生成 token 数。
    pub max_tokens: u32,
    /// 采样温度。
    pub temperature: Option<f32>,
    /// 推理深度级别（目前仅保存，adapter 映射未实现）。
    pub thinking_level: ThinkingLevel,

    // === Tools ===
    /// 活跃工具列表（通常接收已包装的 `HookedTool`）。
    pub tools: Vec<Arc<dyn Tool>>,
    /// 工具的默认执行模式。
    pub default_execution_mode: ToolExecutionMode,

    // === Execution environment ===
    /// 工具执行时使用的执行环境。
    pub env: Arc<dyn ExecutionEnv>,
    /// 中止信号（用于 `ToolContext` 和 loop 的中止检查）。
    pub abort: CancellationToken,

    // === Stream options ===
    /// 传输层配置（目前不直接传递给 adapter，但为将来保留）。
    pub stream_options: StreamOptions,

    // === Required hooks ===
    /// `AgentMessage` → adapter `Message` 转换（必需）。
    pub convert_to_llm: Arc<dyn ConvertToLlmHook>,

    // === Optional hooks ===
    /// LLM 调用前的上下文转换（例如 compaction）。
    pub transform_context: Option<Arc<dyn TransformContextHook>>,
    /// 每个转轮后决定下一转轮的配置。
    pub prepare_next_turn: Option<Arc<dyn PrepareNextTurnHook>>,
    /// 当 LLM 自然停止时，判断是否继续循环。
    pub should_stop: Option<Arc<dyn ShouldStopHook>>,
    /// 在 provider 调用前可编辑 `StreamOptions`。
    pub before_provider_request: Option<Arc<dyn BeforeProviderRequestHook>>,
    /// provider 响应后的观测钩子。
    pub after_provider_response: Option<Arc<dyn AfterProviderResponseHook>>,
    /// 动态身份验证（目前不直接传递给 adapter，但为将来保留）。
    pub auth: Option<Arc<dyn AuthHook>>,

    // === Reactive injection ===
    /// 在转轮间注入的消息（转向）。
    pub steer_rx: Option<mpsc::Receiver<AgentMessage>>,
    /// loop 自然停止后逐个处理的后续消息。
    pub follow_up_rx: Option<mpsc::Receiver<AgentMessage>>,
}

#[cfg(test)]
mod tests {

    #[test]
    #[cfg(feature = "test-utils")]
    fn loop_config_compiles() {
        use crate::test_utils::NoOpEnv;
        let _cfg = LoopConfig {
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
        };
    }
}
