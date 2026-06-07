## 5. `llm-harness`

**职责：** Agent + AgentHarness + Session + Compaction + Skills/Templates。

**依赖：** `llm-harness-types`, `llm-harness-loop`, `tokio`, `serde_json`, `serde_yaml`

> **为什么这么多职责在一个 crate？** AgentHarness 需要 Agent、Session、Compaction、Skills 全部协同工作。将它们拆分为独立 crate 需要引入额外的 trait 抽象来解耦（如 `AgentTrait`、`SessionTrait`）——这是过早抽象。v1 选择单 crate 多模块，内部模块间可以直接引用类型而不需要 trait 中介。如果未来需要独立使用 Agent（不需要 Session），可以通过 `cargo features` 条件编译，而不必拆分 crate。

---

### 5.1 Agent

有状态包装器。包含必要的可观测状态以支持 UI 渲染。

```rust
pub struct Agent {
    client:       Arc<dyn LlmClient>,
    state:        Arc<std::sync::Mutex<AgentState>>,
    event_tx:     tokio::sync::broadcast::Sender<AgentEvent>,
    steer_tx:     tokio::sync::mpsc::Sender<AgentMessage>,
    follow_up_tx: tokio::sync::mpsc::Sender<AgentMessage>,
    abort:        CancellationToken,
}

pub struct AgentState {
    pub phase:             AgentPhase,
    pub model:             String,
    pub model_info:        Option<ModelInfo>,        // 用于 compaction 估算
    pub thinking_level:    ThinkingLevel,
    pub tools:             Vec<Arc<dyn Tool>>,
    pub messages:          Vec<AgentMessage>,
    pub system_prompt:     Option<String>,
    pub streaming_message: Option<AssistantMessage>, // 进行中消息的快照（UI 渲染）
    pub pending_tool_calls: HashSet<String>,         // 正在执行的 tool_use_id 集合
    pub error_message:     Option<String>,           // 最近一次 LLM 失败的错误文本
}

#[derive(PartialEq)]
pub enum AgentPhase { Idle, Running }
```

> **Agent 的设计定位：** Agent 是 "不需要 session/skills 的简化使用场景" 的入口。它管理自己的 messages 数组（不持久化），提供事件广播，支持 steer/follow-up 队列。Agent 是 AgentHarness 的轻量替代——如果你只需要 "给 LLM 发消息、执行 tool、返回结果"，Agent 足够了。
>
> **字段设计理由：**
>
> **`state: Arc<Mutex<AgentState>>`：** 为什么用 `Arc<Mutex<>>` 而非 `RefCell`？Agent 的 `prompt()` 方法内部 spawn task 处理事件流——task 需要访问 state（更新 `streaming_message`、`pending_tool_calls`）。`Arc` 允许 state 在 Agent struct 和内部 task 之间共享。`std::sync::Mutex` 而非 `tokio::sync::Mutex`——见下文锁约束。
>
> **`event_tx: broadcast::Sender`：** 多个订阅者（UI 组件、日志、测试）可以同时监听事件。`broadcast` 的容量默认 256——如果某订阅者消费慢，旧事件会被丢弃。这是有意设计——事件流用于 UI 通知，不是可靠的消息传递。
>
> **`steer_tx` / `follow_up_tx`：** Agent 持有 sender 端，`receiver` 端在每次调用 `agent_loop()` 时通过 `LoopConfig` 传入。Channel 容量由 Agent 的构造参数控制（默认值如 32）。
>
> **`abort: CancellationToken`：** Agent 级别的取消令牌。调用 `abort()` 时触发——内部 task 在每次 `.await` 前检查 token，如果已取消则尽快终止。
>
> **`AgentState` 的可观测字段：**
> - `phase`：Idle 还是 Running——UI 据此显示 "等待输入" 或 "处理中" 状态。
> - `streaming_message`：当前正在流式生成的消息的实时快照。每次 `MessageUpdate` 事件更新此字段（持锁，不跨 await）。UI 可以轮询或通过事件更新。
> - `pending_tool_calls`：正在执行的 tool call ID 集合。UI 可以显示 "正在执行 read_file, search..."。
> - `error_message`：最近一次失败的 LLM 调用的错误文本。保留到下一次 prompt 开始前清空。
>
> **为什么 `model` 是 `String` 而非 `ModelInfo`？** `model` 是发送给 LLM API 的模型标识符（如 `"claude-sonnet-4-6"`）。`model_info` 是此模型的元数据（context_window 等）。两者可能不同步——`model` 是用户的当前选择，`model_info` 是此模型的静态属性。分开存储避免在 `set_model` 时强制提供 `ModelInfo`（调用方可能不知道）。

---

**锁使用约束（严格）：** `std::sync::Mutex` **绝不**跨越 `.await`。模式：

```rust
let snapshot = {
    let st = self.state.lock().unwrap();
    TurnSnapshot { /* clone fields */ }
}; // 锁释放
let stream = agent_loop(client, build_ctx(&snapshot, ...), cfg); // await 期间无锁
```

`streaming_message` / `pending_tool_calls` / `error_message` 在事件处理 task 中持锁短暂更新，不跨 await。

> **为什么选择 `std::sync::Mutex` 而非 `tokio::sync::Mutex`？**
> 1. **性能**——`std::sync::Mutex` 的 lock/unlock 是几十纳秒级的 CPU 指令；`tokio::sync::Mutex` 涉及 async 运行时调度，开销高一个数量级。
> 2. **正确性强制**——`std::sync::Mutex` 的持有者不能在 `.await` 时保持锁（否则死锁或 panic）。这正是我们要强制执行的约束——状态更新应该是瞬时的，任何 I/O 都应该在释放锁之后进行。`tokio::sync::Mutex` 允许跨 await 持锁——看似方便，实则纵容了 "持锁期间做 I/O" 的反模式。
> 3. **Poisoning**——`std::sync::Mutex` 在 panic 时 poison，防止使用可能不一致的状态。Agent 的内部 task panic 不应该静默传播。
>
> **快照模式的实现细节：** 每次 turn 开始前，在持锁块内 clone 需要的字段（model, thinking_level, tools, system_prompt），构造 `TurnSnapshot`，然后释放锁。后续的 `agent_loop()` 调用和整个流式响应期间都不需要锁——loop 使用快照中的不可变数据。Turn 结束后，事件处理 task 短暂持锁更新运行时状态（`streaming_message`、`pending_tool_calls`）。

---

方法分类：

```rust
// === 结构性操作：仅 Idle 阶段；否则 Err(AgentError::NotIdle) ===
pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentError>;
/// 支持直接传入消息序列（如恢复会话）
pub async fn prompt_with_messages(&self, messages: Vec<AgentMessage>) -> Result<(), AgentError>;
/// 从当前 transcript 继续执行（无新输入）——Harness 通过 `agent_loop_continue()` 实现等效功能。
/// Agent 的 `continue_run()` 是独立使用 Agent（不通过 Harness）时的入口。
pub async fn continue_run(&self) -> Result<(), AgentError>;
/// 清空 transcript 与运行时状态（保留 model/tools 等配置）
pub fn reset(&self);

// === 运行时配置（任何阶段；若在 Running 中调用，影响下一轮快照）===
pub fn set_model(&self, model: String, info: Option<ModelInfo>);
pub fn set_thinking_level(&self, level: ThinkingLevel);
pub fn set_tools(&self, tools: Vec<Arc<dyn Tool>>);
pub fn set_system_prompt(&self, prompt: Option<String>);

// === 队列操作：任何阶段均安全 ===
/// 文本便捷形式——内部包装为 UserMessage
pub fn steer(&self, text: impl Into<String>);
pub fn follow_up(&self, text: impl Into<String>);
/// 直接注入完整消息（支持多模态、含图片等）
pub fn steer_message(&self, msg: AgentMessage);
pub fn follow_up_message(&self, msg: AgentMessage);
pub fn clear_steering_queue(&self);
pub fn clear_follow_up_queue(&self);
pub fn clear_all_queues(&self);
pub fn has_queued_messages(&self) -> bool;
pub fn abort(&self);

// === 观测 ===
pub fn state(&self) -> AgentState;  // 快照 clone
pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent>;
pub async fn wait_for_idle(&self);
```

> **方法分类的设计理由：**
>
> **结构性操作（仅 Idle）：** `prompt`、`prompt_with_messages`、`continue_run`、`reset`——改变 agent 的核心状态。在 `Running` 时调用返回 `Err(NotIdle)`——调用方需要自己实现重试或排队。这是最小接口原则——Agent 不内置排队策略，调用方可以自由选择（FIFO 队列、覆盖最新、优先级队列）。
>
> **`prompt_with_messages` 的用途：** 允许调用方直接注入 `Vec<AgentMessage>` 而非纯文本。用于 "从 session log 恢复对话" 场景——调用方从 session 中读取消息历史，作为 `AgentMessage` 序列传入，agent 继续执行。这比让 Agent 自己读取 session 更灵活——调用方控制恢复策略（恢复多少条、是否跳过某些消息）。
>
> **`continue_run` 的用途：** 从当前 transcript 的最后一条消息继续——不需要新输入。`AgentHarness` 的 `prepare_next_turn` 依赖此机制：turn 结束后，Harness 从 session 重建上下文，调用 `continue_run`（而非 `prompt`）让 LLM 继续处理。Loop 层对应的函数是 `agent_loop_continue`。
>
> **`reset` 的设计：** 清空 messages、streaming_message、pending_tool_calls、error_message，但**保留** model、tools、system_prompt 等配置。这样调用方可以复用同一个 Agent 实例处理多个独立的对话，不需要重新创建。
>
> **运行时配置（任何阶段安全）：** `set_model`、`set_thinking_level`、`set_tools`、`set_system_prompt`——在 `Running` 期间调用时，变更不会影响当前 turn（因为 turn 已经 clone 了快照），只影响下一轮。这是 "最终一致性" 模型——不需要在调用时检查 phase。
>
> **队列操作（任何阶段安全）：** `steer`/`follow_up` 及其变体。文本版本 `steer(&str)` 是便捷方法——内部构造 `UserMessage` 后走 `steer_message`。这允许简单场景（纯文本 steer）的一行调用，同时保留多模态消息的完整能力。
>
> **`abort` 方法：** 触发 `CancellationToken`（与 `ToolContext.abort` 共享同一个 token——tool 执行期间检查此 token），清空 steer/follow-up 队列，等待 loop 自然退出（正在执行的 tool 收到取消信号后尽快返回 `ToolError::Aborted`）。调用方应在 `abort()` 后 `await wait_for_idle()` 确保完全停止。
>
> **`clear_all_queues()` 语义：** 清空 steer + follow-up 队列。**不**清空 `queued_next_turn`（Agent 无此字段——仅 AgentHarness 有）。Agent 的 `clear_all_queues` 仅影响 steer/follow-up 两个 channel。
>
> **观测方法：** `state()` 返回 `AgentState` 的快照（clone）——避免调用方持锁。`subscribe()` 返回新的 `broadcast::Receiver`——调用方独立 poll，互不干扰。

---

**并发 prompt 行为：** 同时调用 `prompt` 时，state 锁串行化；先获取者将 phase 转为 Running 并继续，后到者读到 Running 返回 `Err(NotIdle)`。Agent 不内置外部排队。

> **为什么不在 Agent 内部排队？** 排队策略是 application-specific 的——有些应用希望 "新的 prompt 覆盖旧的"（如用户在 agent 回复时改了主意重新发送），有些希望 "排队等待"（如批量处理）。Agent 提供最小保证（互斥执行），调用方在外部实现排队。这是 "机制 vs 策略" 分离原则。

---

### 5.2 Turn 快照

详见 §3.7 `TurnSnapshot`。每次 turn 开始 clone 一份，期间 model/tools 修改进入下一轮。

> **快照的生命周期：** (1) `prompt()` 调用 → (2) 持锁构造 `TurnSnapshot` → (3) 释放锁 → (4) `agent_loop()` 使用快照中的 model/tools/system_prompt → (5) turn 结束后检查是否 `set_model`/`set_tools` 被调用过 → 如果是，下一轮开始前重新构造快照。
>
> **为什么 tools 在快照中是 `Vec<Arc<dyn Tool>>`？** 每个 turn 可能需要不同的工具集（通过 `set_tools`/`set_active_tools`）。快照确保 turn 进行中工具集不会变化——即使在工具执行到一半时调用方修改了 tools，当前 turn 的工具调用仍然使用旧列表完成（保证一致性）。
