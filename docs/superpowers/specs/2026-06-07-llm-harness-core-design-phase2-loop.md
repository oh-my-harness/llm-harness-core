## 4. `llm-harness-loop`

**职责：** 纯函数式 agent loop——给定上下文与配置，返回事件流。不持有持久状态。

**依赖：** `llm-harness-types`, `llm-api-adapter`, `tokio`, `tokio-stream`, `futures`

**重导出（必需）：** 下游 `llm-harness` 不直接依赖 `llm-api-adapter`；loop 通过 `pub use` 暴露下游需要的类型：

```rust
// llm-harness-loop/src/lib.rs
pub use llm_api_adapter::{LlmClient, ModelInfo, Message as LlmMessage, StreamEvent};
```

**`LlmClient` 的假定核心签名**（实际定义在 `llm-api-adapter`，此处给出 loop 实现所依赖的最小接口）：

```rust
// 来自 llm-api-adapter（不在本 crate 定义，仅说明 loop 的使用契约）
pub trait LlmClient: Send + Sync {
    fn stream<'a>(
        &'a self,
        model:    &'a str,
        messages: &'a [LlmMessage],
        system:   Option<&'a str>,
        tools:    &'a [ToolDef],
        options:  &'a StreamOptions,
        auth:     Option<&'a AuthInfo>,
    ) -> BoxStream<'a, Result<StreamEvent, LlmError>>;
}

pub enum StreamEvent {
    MessageStart { id: String, model: String, provider: String, api: String },
    TextDelta    { text: String },
    ThinkingDelta { thinking: String, signature: Option<String> },
    ToolUseStart { id: String, name: String },
    ToolUseDelta { id: String, partial_input: String },
    ToolUseEnd   { id: String, input: serde_json::Value },
    MessageEnd   { stop_reason: StopReason, usage: TokenUsage },
    Error(LlmError),
}

pub struct ToolDef { pub name: String, pub description: String, pub parameters: serde_json::Value }
```

如 llm-api-adapter 的实际接口与此存在偏差，loop 内做适配。

### 4.1 `ConvertToLlmHook`（定义在此 crate，因依赖 `llm-api-adapter`）

```rust
/// AgentMessage → llm-api-adapter::Message 的转换。LoopConfig 必需。
pub trait ConvertToLlmHook: Send + Sync {
    fn convert<'a>(
        &'a self,
        messages: &'a [AgentMessage],
    ) -> BoxFuture<'a, Result<Vec<llm_api_adapter::Message>, AgentError>>;
}

/// 框架提供的默认转换器：
/// - User / Assistant / ToolResult → 直接映射
/// - BranchSummary / CompactionSummary → 带前缀的 system-like UserMessage
/// - Custom → 返回 Err（强制调用方覆盖；或用 `with_custom_converter` 注入处理）
pub struct DefaultConvertToLlm {
    pub custom_handler: Option<Arc<dyn CustomMessageConverter>>,
}

pub trait CustomMessageConverter: Send + Sync {
    fn convert<'a>(&'a self, msg: &'a CustomMessage)
        -> BoxFuture<'a, Result<llm_api_adapter::Message, AgentError>>;
}

impl DefaultConvertToLlm {
    pub fn new() -> Self { Self { custom_handler: None } }
    pub fn with_custom_converter(mut self, c: Arc<dyn CustomMessageConverter>) -> Self {
        self.custom_handler = Some(c); self
    }
}

impl ConvertToLlmHook for DefaultConvertToLlm { /* ... */ }
```

### 4.2 LoopConfig 与 API

```rust
pub struct LoopConfig {
    pub tools:                  Vec<Arc<dyn Tool>>,
    pub default_execution_mode: ToolExecutionMode,
    pub stream_options:         StreamOptions,

    /// 必需
    pub convert_to_llm:         Arc<dyn ConvertToLlmHook>,

    /// 可选 hooks
    pub transform_context:      Option<Arc<dyn TransformContextHook>>,
    pub prepare_next_turn:      Option<Arc<dyn PrepareNextTurnHook>>,
    pub should_stop:            Option<Arc<dyn ShouldStopHook>>,
    pub before_provider_request: Option<Arc<dyn BeforeProviderRequestHook>>,
    pub after_provider_response: Option<Arc<dyn AfterProviderResponseHook>>,
    pub auth:                   Option<Arc<dyn AuthHook>>,

    /// 响应式注入 channels——载荷为完整 AgentMessage 以支持多模态
    pub steer_rx:               Option<tokio::sync::mpsc::Receiver<AgentMessage>>,
    pub follow_up_rx:           Option<tokio::sync::mpsc::Receiver<AgentMessage>>,
}

/// 从初始上下文开始执行
pub fn agent_loop(
    client: Arc<dyn LlmClient>,
    ctx:    AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send;

/// 从当前上下文继续执行（无新 prompt，适合 prepare_next_turn 触发）
pub fn agent_loop_continue(
    client: Arc<dyn LlmClient>,
    ctx:    AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send;
```

**LoopConfig 与 HarnessHooks 的关系（消除重复的真相）：**

- `LoopConfig` 是 loop 层的**直接 API**。不通过 Harness 的调用方（如希望自行编排会话的低层用户）直接构造 `LoopConfig`
- `AgentHarness` 内部维护 `HarnessHooks`，每次启动 loop 时**根据 HarnessHooks 与当前状态构造 LoopConfig**：
  - `convert_to_llm` / `transform_context` / `prepare_next_turn` / `should_stop` / `before_provider_request` / `after_provider_response` / `auth` 直接复制
  - `tools` 从 `HarnessState.tools` + `active_tools` 过滤，并用 `HookedTool` 包装注入 `before_tool_call` / `after_tool_call`
  - `steer_rx` / `follow_up_rx` 从 Harness 自己持有的 channel sender 派生 receiver
- 调用方**不应**在 Harness 已设置 HarnessHooks 时再手动构造 LoopConfig；Harness 的 API 完全屏蔽 LoopConfig

`BeforeToolCallHook` / `AfterToolCallHook` **只在 Harness 中存在**——Loop 层完全没有这两个字段，避免重复。

### 4.3 HookedTool（Loop 与 Harness 的 hook 桥梁）

Harness 通过 `HookedTool` 在每个 tool 上挂载 `before_tool_call` / `after_tool_call`：

```rust
/// 包装一个 Tool，在 execute 时调用 before/after 钩子。
/// Harness 在每次启动 loop 前为每个工具创建一个 HookedTool 实例。
pub struct HookedTool {
    pub inner:        Arc<dyn Tool>,
    pub before:       Option<Arc<dyn BeforeToolCallHook>>,
    pub after:        Option<Arc<dyn AfterToolCallHook>>,
    pub turn_index:   u32,
    pub assistant_message: Arc<AssistantMessage>,  // 由 Harness 在 turn 开始注入
}

impl Tool for HookedTool {
    fn name(&self) -> &str { self.inner.name() }
    fn description(&self) -> &str { self.inner.description() }
    fn parameters_schema(&self) -> &serde_json::Value { self.inner.parameters_schema() }
    fn execution_mode(&self) -> ToolExecutionMode { self.inner.execution_mode() }
    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a ToolContext)
        -> BoxFuture<'a, Result<ToolResult, ToolError>>
    {
        Box::pin(async move {
            let effective_args = if let Some(h) = &self.before {
                match h.on_call(BeforeToolCallCtx { /* ... */ }).await {
                    BeforeToolCallDecision::Allow => args,
                    BeforeToolCallDecision::Modify(a) => a,
                    BeforeToolCallDecision::Deny(r) => return Ok(r),
                }
            } else { args };
            let result = self.inner.execute(effective_args.clone(), ctx).await;
            if let Some(h) = &self.after {
                h.on_complete(AfterToolCallCtx { /* result: &result, ... */ }).await;
            }
            result
        })
    }
}
```

### 4.2 Tool batch 执行：分治调度

按 LLM 返回顺序，以 `Sequential` tool 为分割点切分子组：组内并发，子组间顺序。

```
LLM 返回: [P1, P2, S1, P3, P4, S2, P5]
执行:
  join_all(P1, P2) → 单独 S1 → join_all(P3, P4) → 单独 S2 → P5
```

- 默认 mode 由 `LoopConfig.default_execution_mode` 提供，tool 自身 `execution_mode()` 覆盖默认
- 并发组内任一 tool 失败不影响同组其他 tool（结果原样返回给 LLM）

### 4.3 Steering vs Follow-up

- **Steer**：tool batch 完成后、下一次 LLM 调用之前，channel 中**所有**待处理消息按 FIFO **全部**作为 user 消息注入
- **Follow-up**：agent 自然停止后，从 channel 取**一条**触发新一轮；其余保留等待下次

### 4.4 Stop 优先级

```
LLM stop_reason = ToolUse:
  → 执行 tool batch；若所有 tool 返回 terminate=true：停止 loop
  → 否则进入下一轮（LLM 主导继续）

LLM stop_reason ∈ {EndTurn, MaxTokens, StopSequence, Other}:
  → 询问 should_stop（如配置）：
      true:  停止 loop → 进入 follow_up 检查
      false: 强制再跑一轮（沿用 prepare_next_turn 返回的 context）
  → 未配置 should_stop：停止 loop

abort 信号: 任何时候立即终止，发出 Error(Aborted) + AgentEnd
```

`should_stop` 仅在 LLM 已自然停止时被询问，不能强制中断进行中的 turn——中断走 `abort()`。

### 4.5 事件传递

Loop 返回 cold `Stream`——只有被 poll 才推进。调用方决定何时消费即决定何时推进。框架层无需结算保证。
