## 4. `llm-harness-loop`

**职责：** 纯函数式 agent loop——给定上下文与配置，返回事件流。不持有持久状态。

**依赖：** `llm-harness-types`, `llm-api-adapter`, `tokio`, `tokio-stream`, `futures`

> **"纯函数式"的含义：** `agent_loop()` 是一个纯异步函数——输入是 `Arc<dyn LlmClient>` + `AgentContext` + `LoopConfig`，输出是 `impl Stream<Item = AgentEvent>`。它不访问全局状态、不写文件、不持有内部可变状态。所有的状态（当前上下文、model 配置、工具列表）通过参数传入，通过事件流出。这使得 loop 天然可测试——给定相同的输入和 mock client，输出的事件序列是确定性的。

**StreamEvent → AgentEvent 映射表：** loop 内部将 `LlmClient` 返回的 `StreamEvent` 转换为 `AgentEvent`：

| `StreamEvent` | `AgentEvent` | 备注 |
|---|---|---|
| `MessageStart { id, model, provider, api }` | `MessageStart { message_id: id }` ，同时记录 model/provider/api 用于后续 `AssistantMessage` 构造 | model/provider/api 最终进入 MessageEnd 的 AssistantMessage 字段 |
| `TextDelta { text }` | `TextDelta { message_id, text }` | message_id 关联当前 active message |
| `ThinkingDelta { thinking, signature }` | `ThinkingDelta { message_id, thinking, signature }` | signature 透传，None 亦可 |
| `ToolUseStart { id, name }` | `ToolCallStart { message_id, tool_use_id: id, name }` | |
| `ToolUseDelta { id, partial_input }` | `ToolCallArgsDelta { tool_use_id: id, partial_input }` | |
| `ToolUseEnd { id, input }` | `ToolCallEnd { tool_use_id: id, args: input }` | |
| `MessageEnd { stop_reason, usage }` | `MessageEnd { message_id, message: AssistantMessage { stop_reason, usage, model, provider, api, ... } }` | 在此构造完整的 AssistantMessage |
| `Error(e)` | `Error(AgentError::Provider(e.to_string()))` | follow by AgentEnd |

**`agent_loop_continue` 的初始 steer 处理：** 若 loop 启动前 steer channel 中已有消息（前一轮遗留），`agent_loop_continue` 在第一轮 LLM 调用前执行一次 steer poll，清空所有 pending steer 消息。这避免前一轮的 steer 在 continue 场景中被意外注入（continue 应仅处理 follow-up，steer 属于上一轮已结束的上下文）。

**重导出（必需）：** 下游 `llm-harness` 不直接依赖 `llm-api-adapter`；loop 通过 `pub use` 暴露下游需要的类型：

```rust
// llm-harness-loop/src/lib.rs
pub use llm_api_adapter::{LlmClient, ModelInfo, Message as LlmMessage, StreamEvent};
```

> **为什么重导出而非让 harness 直接依赖 llm-api-adapter？** 依赖图纯洁性。harness 的依赖声明是 `llm-harness-loop`——它使用 loop 暴露的类型（`LlmClient`、`ModelInfo`），而不需要知道这些类型来自 `llm-api-adapter`。如果未来更换 LLM adapter（如 `llm-api-adapter-v2`），只需要改 loop 的重导出，harness 不需要改动。

---

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

> **为什么在此给出假定签名？** `llm-api-adapter` 的接口可能还在演进中。loop 需要的核心能力是明确的——将消息列表发给 LLM，获取流式事件。即使实际接口与此有偏差（如参数顺序不同、返回类型不同），适配层可以在 loop 内部的 `stream_assistant_response` 函数中完成，不影响 loop 的主体逻辑。
>
> **关于签名中的 `StreamOptions` 和 `AuthInfo`：** 上述签名是为说明 loop 对 adapter 需求的**最小契约**，使用了与 `llm-harness-types` 同名的类型。实际上 `llm-api-adapter` 有自己的类型定义——loop 调用 `client.stream()` 前需要将 harness 侧类型转换为 adapter 侧类型，详见 §4.8。
>
> **`StreamEvent` 的变体对应关系：**
> - `MessageStart` → loop 发出 `MessageStart` + 可选 `TextDelta` 等 token 事件
> - `TextDelta` / `ThinkingDelta` → 直接映射为 `AgentEvent::TextDelta` / `ThinkingDelta`
> - `ToolUseStart` / `ToolUseDelta` / `ToolUseEnd` → 映射为 `ToolCallStart` / `ToolCallArgsDelta` / `ToolCallEnd`
> - `MessageEnd` → loop 发出 `MessageEnd`（携带完整的 `AssistantMessage`），然后检查 `stop_reason`
> - `Error` → loop 发出 `AgentEvent::Error`
>
> **`ToolDef` 与 `Tool` trait 的关系：** `ToolDef` 是发送给 LLM API 的 JSON schema 表示（扁平数据），`Tool` trait 是框架内的工具抽象（含 execute 行为）。loop 在构造 LLM 请求时，将 `Arc<dyn Tool>` 列表转换为 `Vec<ToolDef>`。

---

### 4.1 `ConvertToLlmHook`（定义在此 crate，因依赖 `llm-api-adapter`）

> **为什么不在 types crate？** 此 trait 的 `convert` 方法返回 `Vec<llm_api_adapter::Message>`，引用了外部类型。types 是零 IO 层，不应依赖 `llm-api-adapter`。定义在 loop crate 中，因为 loop 同时依赖 types 和 llm-api-adapter——它是两个类型世界的桥梁。

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

> **为什么 `ConvertToLlmHook` 在 `LoopConfig` 中是必需的？** AgentMessage 的某些变体（`BranchSummary`、`CompactionSummary`、`Custom`）不能直接发送给 LLM——API 不认识这些消息角色。必须有一个转换步骤将它们变为 LLM API 可接受的 `UserMessage`。loop 不做假设——它要求调用方提供转换器。
>
> **`DefaultConvertToLlm` 的设计：**
> - 对标准消息（User/Assistant/ToolResult）：直接映射——字段名和结构一一对应。
> - 对摘要消息（BranchSummary/CompactionSummary）：包装为 `<summary>...</summary>` 格式的 UserMessage——LLM 看到的是 "以下是对话历史的摘要..."。
> - 对 Custom 消息：返回 `Err`——因为框架不知道如何转换应用层自定义消息。调用方必须通过 `with_custom_converter` 注入自定义转换逻辑。
>
> **为什么用 builder 模式（`with_custom_converter`）而非 trait 的 default 方法？** Custom 消息的转换是 application-specific 的——不能提供合理的默认实现。`builder` 模式让调用方显式意识到 "我需要处理 Custom 消息"，而不是编译通过后在运行时才发现 Err。

---

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
```

> **设计理由（逐字段）：**
>
> **`tools`**：Vec 而非 HashSet——工具顺序可能重要（某些 LLM 对 tool definition 的顺序敏感）。`Arc<dyn Tool>` 允许工具实例在多个 turn 之间共享（不 clone 整个 tool 对象）。
>
> **`default_execution_mode`**：当 tool 自身的 `execution_mode()` 返回默认值时（绝大多数 tool 不覆盖此方法），使用此配置作为 fallback。允许调用方全局切换 "全部并行" vs "全部顺序"。
>
> **`stream_options`**：直接传递给 `LlmClient::stream`。包括 timeout、retry、headers、metadata、cache 配置。
>
> **`convert_to_llm`（必需）**：没有此 hook，loop 无法将 AgentMessage 转为 LLM API 格式。必需性的强制——如果缺失，loop 在构造时就 panic（或 `new` 返回 `Err`）。
>
> **可选 hooks**：全部为 `Option<Arc<dyn Trait>>`。`None` 表示 "使用默认行为"——loop 在调用前检查 `is_some()`，None 时跳过。
>
> **`steer_rx` / `follow_up_rx`**：channel 接收端。`Option` 允许不启用响应式注入。使用 `AgentMessage` 而非 `String`——steer 消息可能包含图片等多模态内容。
>
> **LoopConfig 是消耗型（owned）而非 borrow 型：** 函数签名 `agent_loop(config: LoopConfig)` 而非 `config: &LoopConfig`。这允许 loop 在内部解构 config——将 `tools` move 到内部状态，将 `steer_rx` move 到 poll 循环中。避免不必要的 clone。

---

```rust
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

> **为什么两个函数而非一个带 flag 的函数？**
> - `agent_loop`：发出 `AgentStart` 事件（含 initial_messages），为 prompt messages 发出 `message_start`/`message_end` 事件，然后将它们加入上下文。
> - `agent_loop_continue`：也发出 `AgentStart`（`initial_messages` 为空），但不注入新消息——直接从上下文的最后一条消息开始。用于 Harness 的 `prepare_next_turn` 触发的新一轮执行。
>
> **为什么返回 `impl Stream` 而非具体类型？** (1) 隐藏内部实现——loop 的内部状态机不是公开 API；(2) 允许未来优化（如从手动实现 Stream 改为 async generator）；(3) `+ Send` 确保 stream 可以跨 tokio task 传递。
>
> **`client: Arc<dyn LlmClient>`**：Arc 而非 Box——同一个 client 实例被 loop 内部多次调用（每轮 LLM 请求），且调用方可能共享同一个 client 给多个 Agent/Harness 实例。

---

**LoopConfig 与 HarnessHooks 的关系（消除重复的真相）：**

> - `LoopConfig` 是 loop 层的**直接 API**。不通过 Harness 的调用方（如希望自行编排会话的低层用户）直接构造 `LoopConfig`
> - `AgentHarness` 内部维护 `HarnessHooks`，每次启动 loop 时**根据 HarnessHooks 与当前状态构造 LoopConfig**：
>   - `convert_to_llm` 从 `AgentHarness.convert_to_llm` 字段（独立字段，不在 HarnessHooks 中）
>   - `auth` 从 `AgentHarness.auth` 字段（独立字段，不在 HarnessHooks 中）
>   - `transform_context` / `prepare_next_turn` / `should_stop` / `before_provider_request` / `after_provider_response` 从 HarnessHooks 复制
>   - `tools` 从 `HarnessState.tools` + `active_tools` 过滤，并用 `HookedTool` 包装注入 `before_tool_call` / `after_tool_call`
>   - `steer_rx` / `follow_up_rx` 从 Harness 自己持有的 channel sender 派生 receiver（channel 容量默认 256；满时 sender 阻塞，调用方的 steer()/follow_up() 感知背压）
> - 调用方**不应**在 Harness 已设置 HarnessHooks 时再手动构造 LoopConfig；Harness 的 API 完全屏蔽 LoopConfig
>
> **为什么 HarnessHooks 和 LoopConfig 分开？** HarnessHooks 是 "存储格式"——保存在 Harness 中，跨多次 prompt 调用复用。LoopConfig 是 "传输格式"——每次调用 `agent_loop()` 时从 HarnessHooks 构造，包含当前 turn 的特定信息（tools 过滤、steer channel 派生）。这种分离避免了在 Harness 中存储 "临时状态"（如某次 turn 的 steer receiver）。

`BeforeToolCallHook` / `AfterToolCallHook` **只在 Harness 中存在**——Loop 层完全没有这两个字段，避免重复。

---

### 4.3 HookedTool（Loop 与 Harness 的 hook 桥梁）

Harness 通过 `HookedTool` 在每个 tool 上挂载 `before_tool_call` / `after_tool_call`：

```rust
/// 包装一个 Tool，在 execute 时调用 before/after 钩子。
/// Harness 在每次启动 loop 前为每个工具创建一个 HookedTool 实例。
/// 无状态——turn_index 和 assistant_message 均通过 ToolContext 传入，不存储在结构体中。
pub struct HookedTool {
    pub inner:  Arc<dyn Tool>,
    pub before: Option<Arc<dyn BeforeToolCallHook>>,
    pub after:  Option<Arc<dyn AfterToolCallHook>>,
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
                match h.on_call(BeforeToolCallCtx {
                    assistant_message: &ctx.assistant_message,
                    turn_index: ctx.turn_index,
                    /* tool_use_id, tool_name, args, ... */
                }).await {
                    BeforeToolCallDecision::Allow => args,
                    BeforeToolCallDecision::Modify(a) => a,
                    BeforeToolCallDecision::Deny(r) => return Ok(r),
                }
            } else { args };
            let result = self.inner.execute(effective_args.clone(), ctx).await;
            if let Some(h) = &self.after {
                let decision = h.on_complete(AfterToolCallCtx {
                    assistant_message: &ctx.assistant_message,
                    turn_index: ctx.turn_index,
                    /* result: &result, ... */
                }).await;
                if let AfterToolCallDecision::Patch(patch) = decision {
                    return Ok(apply_patch(result, patch));
                }
            }
            result
        })
    }
}
```

> **设计理由：**
>
> **Decorator 模式：** HookedTool 实现了 `Tool` trait，对外部（loop）来说它就是一个普通的 Tool。loop 不需要知道 hook 的存在——它只调用 `tool.execute()`。Hook 的注入是透明的。
>
> **为什么 `assistant_message` 和 `turn_index` 不存储在 HookedTool 中？** `assistant_message` 不是工具的属性，而是本次 tool call 的上下文——它在 `build_loop_config()` 时（loop 启动前）根本不存在，只有 loop 内部处理 `MessageEnd` 后才完整。将它放在 `HookedTool` 结构体意味着要么延迟注入（`Arc<RwLock<Option<...>>>`，额外管理负担），要么 per-turn 重建 `HookedTool`（打破 loop 的纯函数特性）。正确的位置是 `ToolContext`：loop 在 `MessageEnd` 后持有完整的 `AssistantMessage`，构造 `ToolContext` 时直接传入。`HookedTool.execute` 从 `ctx.assistant_message` 和 `ctx.turn_index` 读取，再填入 `BeforeToolCallCtx` / `AfterToolCallCtx`——无共享可变状态，`HookedTool` 保持无状态纯 decorator。
>
> **execute 中的逻辑流程：** (1) 调用 before hook → Allow/Modify/Deny，hook ctx 中的 `assistant_message` 和 `turn_index` 来自 `ctx`；(2) 如果 Deny，直接返回 hook 提供的 ToolResult（不执行内部 tool）；(3) 否则用 (可能被修改的) args 执行内部 tool；(4) 调用 after hook，传入结果；(5) after hook 可返回 `AfterToolCallDecision::Patch` 覆盖结果，或 `Passthrough` 保持原样；(6) 返回最终结果。
>
> **`effective_args.clone()` 的必要性：** `execute` 的 `args` 参数是 owned `serde_json::Value`——如果 before hook 返回 Allow，我们仍需要原始的 args。但 Modify 返回了新 args，所以不需要 clone 原始的。clone 只发生在 Allow 路径——这是正确的折中。
>
> **定义位置（loop crate 而非 harness）：** HookedTool 实现了 `Tool` trait——这个 trait 是 types crate 定义的。将 HookedTool 放在 loop crate 让它可以被 loop 的测试直接使用，且 harness 依赖 loop 是正常的依赖方向。

---

### 4.4 Tool batch 执行：分治调度

> **这是 v5 设计的关键改进——从 "全退顺序" 到 "分治调度"。**

按 LLM 返回顺序，以 `Sequential` tool 为分割点切分子组：组内并发，子组间顺序。

```
LLM 返回: [P1, P2, S1, P3, P4, S2, P5]
执行:
  join_all(P1, P2) → 单独 S1 → join_all(P3, P4) → 单独 S2 → P5
```

- 默认 mode 由 `LoopConfig.default_execution_mode` 提供，tool 自身 `execution_mode()` 覆盖默认
- 并发组内任一 tool 失败不影响同组其他 tool（结果原样返回给 LLM）

> **为什么分治而非全局顺序/全局并发？**
> - **全局顺序**：5 个并发 tool + 1 个 sequential tool = 全部 6 个顺序执行 → P1-P5 被 S1 无辜拖慢
> - **全局并发**：忽略 sequential 标记 → 可能导致文件写入竞争
> - **分治**：只将 batch 按 Sequential 切分，组内并发组间顺序——最优的并行度
>
> **并发组内失败的隔离：** `join_all` 等待所有 future 完成——即使其中一个返回 `Err`，其他的仍正常完成。每个 tool 的结果独立返回给 LLM——LLM 看到 "工具 A 成功，工具 B 失败"，可以据此调整后续行为。这是 Anthropic/OpenAI tool calling 的标准行为。
>
> **顺序组内的失败：** 如果某个 Sequential tool 失败，后续子组**仍然执行**——原因是 LLM 在发出 tool calls 时已经假设所有 tool 都会执行。跳过后续 tool 会导致 LLM 看到 "有些 tool 没有结果"——比 "有些 tool 失败了" 更难处理。

---

### 4.5 Steering vs Follow-up

- **Steer**：tool batch 完成后、下一次 LLM 调用之前，channel 中**所有**待处理消息按 FIFO **全部**作为 user 消息注入
- **Follow-up**：agent 自然停止后，从 channel 取**一条**触发新一轮；其余保留等待下次

> **Steer 的 "全部注入" 语义：** 如果用户在 tool 执行期间连续发送了 3 条 steer 消息（"停！" "换方向" "看这里"），全部注入让 LLM 看到完整的用户意图演变。相比之下，只取一条可能丢失上下文。
>
> **Follow-up 的 "一次一条" 语义：** 多条 follow-up 消息通常是独立的后续任务（"现在测试一下"、"写个 README"）——逐条处理允许 LLM 在每条之间执行 tool calls 并自然停止。如果全部注入，LLM 可能混淆多个独立任务。
>
> **为什么不用 QueueMode（TS 的 `"all" | "one-at-a-time"`）？** v1 简化——这两种策略在实践中最常用，且各自对应明确的语义场景。如果后续需要更灵活的队列策略，可以加 `QueueMode` 枚举作为 LoopConfig 的可选字段。

---

### 4.6 Stop 优先级

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

> **优先级的逻辑链：**
> 1. **Abort 最高优先级**——用户取消压倒一切，任何时刻立即终止。Loop 在关键 await 点检查 `CancellationToken::is_cancelled()`。
> 2. **Terminate 次高**——tool 宣告任务完成。这反映了 "tool 比 LLM 更了解任务状态" 的设计信念。例如 deploy tool 部署成功后，不需要再问 LLM "下一步做什么"。
> 3. **ShouldStop 再次**——LLM 表达了停止意图，调用方有机会覆盖（如自动继续被截断的生成）。
> 4. **LLM 意图最后**——如果 LLM 返回 `EndTurn` 且没有配置 should_stop，则照常停止。
>
> **`should_stop` 的约束：** 仅在 LLM 已自然停止时被询问，不能强制中断进行中的 turn——中断走 `abort()`。这个约束防止了 "hook 在 tool 执行到一半时就要求停止" 的混乱状态。

---

### 4.7 事件传递

Loop 返回 cold `Stream`——只有被 poll 才推进。调用方决定何时消费即决定何时推进。框架层无需结算保证。

> **Cold Stream vs Hot Stream 的选择理由：**
> - **Cold Stream（当前选择）：** `agent_loop()` 返回的 Stream 在被 poll 之前不做任何事。调用方控制轮询节奏——如果调用方暂停消费（如 UI 在处理前一个事件），loop 自然暂停。这避免了 "事件生产快于消费导致无界缓冲" 的问题。
> - **Hot Stream（不采用）：** 如果 loop 内部 spawn task 并通过 channel 推送事件，需要处理背压（bounded channel 满时怎么办？block？drop？）。Cold Stream 将背压自然地转化为 "调用方不 poll"。
>
> **"框架层无需结算保证"的原因：** 在 TS 中，`agent_end` 的监听器是 fire-and-forget（异步但不等 await）——框架需要显式 `await` 所有监听器完成才算 "settled"。Rust 的 `broadcast::Receiver` 是拉取式——调用方 poll 自己的 receiver，框架不需要知道谁在监听。当 `agent_loop` 返回的 Stream 结束时，所有事件都已经发出——不需要额外的 settled 确认。

---

### 4.8 llm-api-adapter 集成边界

Loop crate 是 `llm-harness-types` 与 `llm-api-adapter` 之间的**类型桥梁**。§4 开头的假定签名中 `options: &StreamOptions` 和 `auth: Option<&AuthInfo>` 写的是与 `llm-harness-types` 同名的类型——实际上 `llm-api-adapter` 定义了自己的参数类型，loop 在调用 `client.stream()` 前需要将 harness 侧类型转换为 adapter 侧类型。

---

#### 镜像类型与转换方向

| llm-harness-types 侧 | 转换方向 | llm-api-adapter 侧 | 备注 |
|---|---|---|---|
| `AuthInfo` | loop 转出 → | adapter 自有 auth 格式 | 来自 `AuthHook::resolve()` |
| `StreamOptions` + `ThinkingLevel` | loop 转出 → | adapter 自有调用参数 | `BeforeProviderRequestHook` 可修改后再转 |
| adapter `StopReason` | → loop 转入 | `llm_harness_types::StopReason` | 来自 `MessageEnd` |
| adapter `TokenUsage` | → loop 转入 | `llm_harness_types::TokenUsage` | 来自 `MessageEnd` |
| `Vec<AgentMessage>` | `ConvertToLlmHook` → | `Vec<adapter::Message>` | hook 定义在本 crate，见 §4.1 |

**Loop 内部私有转换函数（不对外暴露）：**

```rust
fn tools_to_defs(tools: &[Arc<dyn Tool>]) -> Vec<adapter::ToolDef>;
fn convert_auth(info: &AuthInfo) -> adapter::AuthParams;
fn convert_stream_opts(opts: &StreamOptions, level: ThinkingLevel) -> adapter::StreamOptions;
fn convert_stop_reason(r: adapter::StopReason) -> StopReason;
fn convert_usage(u: adapter::Usage) -> TokenUsage;
```

> **为什么这些函数是私有的？** 它们是实现细节——绑定于当前 `llm-api-adapter` 版本的参数结构。如果 adapter 演进（字段名、结构变化），只需更新这些函数，不影响 loop 的公开 API。对外暴露转换函数会把 adapter 的内部类型泄漏到 loop 的公开接口中。

---

#### Tool → ToolDef 转换

```rust
fn tools_to_defs(tools: &[Arc<dyn Tool>]) -> Vec<adapter::ToolDef> {
    tools.iter().map(|t| adapter::ToolDef {
        name:        t.name().to_owned(),
        description: t.description().to_owned(),
        parameters:  t.parameters_schema().clone(),
    }).collect()
}
```

`parameters_schema()` 返回 `&serde_json::Value`，原样转发给 adapter——调用方（工具实现者）负责提供合法的 JSON Schema 对象（含 `type: "object"`, `properties`, 可选 `required`）。Loop 不做内容校验。

> **为什么每次 turn 都重新转换而非缓存？** 工具列表在 turn 之间可能被 `set_tools()`/`set_active_tools()` 修改（通过 `PrepareNextTurnHook` 注入最新 HarnessState）。重新转换保证每次 LLM 请求都使用最新工具定义，代价是每次的浅克隆——相对于网络 RTT 可忽略不计。

---

#### client.stream() 调用组装（每轮 LLM 请求）

```rust
async fn call_llm(
    client:   &dyn LlmClient,
    snapshot: &TurnSnapshot,   // model, thinking_level（turn 开始时克隆，见下文）
    ctx:      &AgentContext,
    config:   &LoopConfig,
) -> BoxStream<Result<adapter::StreamEvent, adapter::LlmError>>
{
    // 1. 消息转换
    let llm_messages = config.convert_to_llm.convert(&ctx.messages).await?;

    // 2. Tool → ToolDef
    let tool_defs = tools_to_defs(&config.tools);

    // 3. BeforeProviderRequestHook 可修改 stream_options
    let mut opts = config.stream_options.clone();
    if let Some(h) = &config.before_provider_request {
        h.before_request(&mut opts).await;
    }

    // 4. 解析 auth 并转换
    let auth_params = match &config.auth {
        Some(h) => Some(convert_auth(&h.resolve().await?)),
        None    => None,
    };

    // 5. 调用 adapter
    client.stream(
        &snapshot.model,
        &llm_messages,
        ctx.system_prompt.as_deref(),
        &tool_defs,
        &convert_stream_opts(&opts, snapshot.thinking_level),
        auth_params.as_ref(),
    )
}
```

**`TurnSnapshot`** 是 turn 开始时从锁保护的状态快照一次克隆的值（model、thinking_level 等），保证 turn 期间的 LLM 调用参数不受运行时 `set_model()` 等调用的影响——变更在下一轮生效。

> **为什么在 `BeforeProviderRequestHook` 之后才转换 opts？** hook 操作的是 `StreamOptions`（harness 类型）——它不应该知道 adapter 的内部格式。转换发生在 hook 之后，确保 hook 看到的是语义清晰的 harness 类型，而 adapter 收到的是它自己的格式。

---

#### StreamEvent → AgentEvent 映射与 AssistantMessage 积累

Loop 消费 `BoxStream<Result<adapter::StreamEvent, adapter::LlmError>>`，维护流式状态并同步发出 `AgentEvent`：

```rust
struct StreamingState {
    message_id: String,
    content:    Vec<ContentBlock>,   // 积累中的内容块列表
    provider:   String,
    api:        String,
    model:      String,
}
```

| adapter::StreamEvent | Loop 动作 | 发出 AgentEvent |
|---|---|---|
| `MessageStart { id, model, provider, api }` | 初始化 `StreamingState` | `AgentEvent::MessageStart { id, model, provider, api }` |
| `TextDelta { text }` | 向当前 `Text` ContentBlock 追加文本（或新建） | `AgentEvent::TextDelta { text }` + `MessageUpdate` |
| `ThinkingDelta { thinking, signature }` | 向当前 `Thinking` ContentBlock 追加内容（或新建） | `AgentEvent::ThinkingDelta { thinking, signature }` + `MessageUpdate` |
| `ToolUseStart { id, name }` | 开始新 `ToolUse` ContentBlock，partial_input = "" | `AgentEvent::ToolCallStart { id, name }` |
| `ToolUseDelta { id, partial_input }` | 追加到对应 ToolUse 的 partial_input 字段 | `AgentEvent::ToolCallArgsDelta { id, delta: partial_input }` |
| `ToolUseEnd { id, input }` | 完成 ToolUse ContentBlock（用完整 `input` 替换 partial_input） | `AgentEvent::ToolCallEnd { id }` |
| `MessageEnd { stop_reason, usage }` | 构造完整 `AssistantMessage`，清空 StreamingState | `AgentEvent::MessageEnd { message }` |
| `Error(e)` | 发出错误事件，终止 stream 消费 | `AgentEvent::Error(AgentError::Provider(e))` |

**`MessageEnd` 时构造完整 AssistantMessage：**

```rust
let message = AssistantMessage {
    content:       state.content,
    stop_reason:   Some(convert_stop_reason(stop_reason)),
    timestamp:     chrono::Utc::now(),
    provider:      Some(state.provider),
    api:           Some(state.api),
    model:         Some(state.model),
    usage:         Some(convert_usage(usage)),
    error_message: None,
};
```

`AfterProviderResponseHook` 在 `MessageEnd` 构造完成后、工具执行前调用，接收 `ProviderResponseInfo`（含 `usage`、`latency_ms`、可选 `status_code`/响应头）用于观测。

> **为什么 AssistantMessage 在 MessageEnd 才完整？** `stop_reason` 和 `usage` 只在 `MessageEnd` 时可知——streaming 期间 model 尚未决定如何结束。Provider、model 字段在 `MessageStart` 已知，但必须等 `MessageEnd` 才能一次性写入完整的 `AssistantMessage` 对象（避免构造半成品再修改）。
>
> **为什么 ThinkingDelta 中的 `signature` 是 `Option<String>`？** Anthropic 在思考内容结束时会随附 signature（用于 extended thinking 验证）；OpenAI/DeepSeek 等不使用此字段，传 `None`。Loop 原样转发——不对 signature 做解释或校验。
