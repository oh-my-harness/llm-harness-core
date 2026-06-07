## 5. `llm-harness`

**职责：** Agent + AgentHarness + Session + Compaction + Skills/Templates。

**依赖：** `llm-harness-types`, `llm-harness-loop`, `tokio`, `serde_json`, `serde_yaml`

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

**锁使用约束（严格）：** `std::sync::Mutex` **绝不**跨越 `.await`。模式：

```rust
let snapshot = {
    let st = self.state.lock().unwrap();
    TurnSnapshot { /* clone fields */ }
}; // 锁释放
let stream = agent_loop(client, build_ctx(&snapshot, ...), cfg); // await 期间无锁
```

`streaming_message` / `pending_tool_calls` / `error_message` 在事件处理 task 中持锁短暂更新，不跨 await。

方法分类：

```rust
// === 结构性操作：仅 Idle 阶段；否则 Err(AgentError::NotIdle) ===
pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentError>;
/// 支持直接传入消息序列（如恢复会话）
pub async fn prompt_with_messages(&self, messages: Vec<AgentMessage>) -> Result<(), AgentError>;
/// 从当前 transcript 继续执行（无新输入）——AgentHarness::next_turn 的基础
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

**并发 prompt 行为：** 同时调用 `prompt` 时，state 锁串行化；先获取者将 phase 转为 Running 并继续，后到者读到 Running 返回 `Err(NotIdle)`。Agent 不内置外部排队。

### 5.2 Turn 快照

详见 §3.7 `TurnSnapshot`。每次 turn 开始 clone 一份，期间 model/tools 修改进入下一轮。
