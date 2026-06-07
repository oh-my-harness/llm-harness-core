### 5.6 AgentHarness

编排层：直接驱动 `agent_loop()`（不内嵌 Agent），自己管理 session 写入、配置变更记录、resource 解析。

```rust
pub struct AgentHarness {
    client:         Arc<dyn LlmClient>,
    session:        Session,
    env:            Arc<dyn ExecutionEnv>,
    skills:         Vec<Skill>,
    templates:      Vec<PromptTemplate>,
    state:          Arc<std::sync::Mutex<HarnessState>>,
    event_tx:       tokio::sync::broadcast::Sender<AgentHarnessEvent>,
    convert_to_llm: Arc<dyn ConvertToLlmHook>,   // 来自 loop crate，独立于 HarnessHooks
    hooks:          HarnessHooks,
    stream_options: StreamOptions,
    auth:           Option<Arc<dyn AuthHook>>,
    // 内部 channels（外部不可见）
    steer_tx:       tokio::sync::mpsc::Sender<AgentMessage>,
    follow_up_tx:   tokio::sync::mpsc::Sender<AgentMessage>,
    abort:          CancellationToken,
}

pub struct HarnessState {
    pub phase:             HarnessPhase,
    pub model:             String,
    pub model_info:        Option<ModelInfo>,
    pub thinking_level:    ThinkingLevel,
    pub tools:             Vec<Arc<dyn Tool>>,
    pub active_tools:      Option<HashSet<String>>,  // None = 全部启用
    pub system_prompt:     Option<String>,
    pub streaming_message: Option<AssistantMessage>,
    pub pending_tool_calls: HashSet<String>,
    pub pending_session_writes: Vec<SessionEntryPayload>, // 运行中延迟写入
    pub queued_next_turn:  Vec<AgentMessage>,           // next_turn 缓冲：下次 prompt 时合并到初始消息
}

// HarnessPhase 定义在 §3.1（types crate）以便 HarnessError::NotIdle 携带

/// HarnessHooks 是 Harness 内 hook 的**唯一真相源**。
/// Harness 不暴露 LoopConfig 给调用方；每次启动 loop 时由 Harness 内部从 HarnessHooks
/// 与 HarnessState 构造一个临时 LoopConfig（详见 §4.2 末尾的翻译规则）。
/// convert_to_llm 不在 HarnessHooks 中——它是 loop crate 定义的 trait，
/// Harness 在 new() 时接受一份 Arc<dyn ConvertToLlmHook> 字段保存，构造 LoopConfig 时填入。
pub struct HarnessHooks {
    pub before_turn:              Option<Arc<dyn BeforeTurnHook>>,
    pub after_turn:               Option<Arc<dyn AfterTurnHook>>,
    pub before_tool_call:         Option<Arc<dyn BeforeToolCallHook>>,
    pub after_tool_call:          Option<Arc<dyn AfterToolCallHook>>,
    pub transform_context:        Option<Arc<dyn TransformContextHook>>,
    pub prepare_next_turn:        Option<Arc<dyn PrepareNextTurnHook>>,
    pub should_stop:              Option<Arc<dyn ShouldStopHook>>,
    pub before_provider_request:  Option<Arc<dyn BeforeProviderRequestHook>>,
    pub after_provider_response:  Option<Arc<dyn AfterProviderResponseHook>>,
    pub before_compact:           Option<Arc<dyn BeforeCompactHook>>,
}
```

**Harness 事件：** 透传 AgentEvent 之外，提供 enum variant 通知配置变更与生命周期，对齐 pi-agent-core 的 20+ hook 事件，但归并为更清晰的 enum：

```rust
pub enum AgentHarnessEvent {
    Agent(AgentEvent),
    PhaseChange       { from: HarnessPhase, to: HarnessPhase },
    ModelUpdate       { from: String, to: String },
    ThinkingLevelUpdate { from: ThinkingLevel, to: ThinkingLevel },
    ToolsUpdate       { added: Vec<String>, removed: Vec<String> },
    ActiveToolsUpdate { active: Option<HashSet<String>> },
    ResourcesUpdate   { skills: usize, templates: usize, diagnostics: Vec<SkillDiagnostic> },
    SessionInfoUpdate { name: String },
    CompactionStart   { estimated_tokens: usize },
    CompactionEnd     { stats: Option<CompactionStats>, error: Option<String> },
    QueueUpdate       { steer_len: usize, follow_up_len: usize },
    SavePoint         { entries_flushed: usize },
    BranchForked      { from: EntryId, new_leaf: EntryId, label: Option<String> },
    BranchSwitched    { from: EntryId, to: EntryId },
    BranchDeleted     { leaf: EntryId },
    BranchSummarized  { leaf: EntryId, summary: String },
    Aborted,
    Settled,
}

pub struct CompactionStats {
    pub tokens_before: usize,
    pub tokens_after:  usize,
    pub compressed_entries: usize,
}
```

**操作 API：**

```rust
// === 结构性：仅 Idle；否则 Err(HarnessError::NotIdle) ===
pub async fn prompt(&self, text: impl Into<String>) -> Result<(), HarnessError>;
pub async fn prompt_with_messages(&self, messages: Vec<AgentMessage>) -> Result<(), HarnessError>;
pub async fn prompt_from_template(&self, name: &str, args: Vec<String>) -> Result<(), HarnessError>;
pub async fn skill(&self, name: &str, additional: Option<&str>) -> Result<(), HarnessError>; // 显式 skill 调用
pub async fn compact(&self) -> Result<CompactionStats, HarnessError>;
pub async fn reload_resources(&self, skill_dirs: Vec<PathBuf>, template_dirs: Vec<PathBuf>)
    -> Result<(), HarnessError>;

// === 运行时配置（任何阶段；自动追加 session entry 记录变更）===
pub async fn set_model(&self, model: String, info: Option<ModelInfo>) -> Result<(), HarnessError>;
pub async fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), HarnessError>;
pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) -> Result<(), HarnessError>;
pub async fn set_active_tools(&self, active: Option<HashSet<String>>) -> Result<(), HarnessError>;
pub async fn set_session_name(&self, name: String) -> Result<(), HarnessError>;

// === Session 直操作 ===
pub async fn append_message(&self, msg: AgentMessage) -> Result<EntryId, HarnessError>;
pub async fn append_custom_entry(&self, r#type: String, data: serde_json::Value)
    -> Result<EntryId, HarnessError>;

// === 分支操作（仅 Idle）===
/// 在活跃 leaf 当前历史的某 entry 上 fork 新分支并切换为活跃 leaf
pub async fn fork_branch(&self, from_entry: EntryId, label: Option<String>)
    -> Result<EntryId, HarnessError>;
/// 切换活跃 leaf 到指定 entry（目标必须是 leaf）
pub async fn navigate_tree(&self, target: EntryId) -> Result<(), HarnessError>;
/// 列出所有分支
pub async fn list_branches(&self) -> Result<Vec<BranchInfo>, HarnessError>;
/// 删除分支
pub async fn delete_branch(&self, leaf: EntryId) -> Result<(), HarnessError>;
/// 为指定 leaf 生成 AI 摘要（用 summary_model 调用 LLM），写入 BranchSummary entry
pub async fn generate_branch_summary(&self, leaf: EntryId)
    -> Result<BranchSummaryEntry, HarnessError>;

// === 队列：任何阶段均安全 ===
pub fn steer(&self, text: impl Into<String>);
pub fn follow_up(&self, text: impl Into<String>);
pub fn next_turn(&self, text: impl Into<String>);  // turn 边界注入
pub fn clear_steering_queue(&self);
pub fn clear_follow_up_queue(&self);
pub fn clear_all_queues(&self);
pub fn has_queued_messages(&self) -> bool;
pub fn abort(&self);

// === 观测 ===
pub fn state(&self) -> HarnessState;
pub fn skills(&self) -> &[Skill];
pub fn templates(&self) -> &[PromptTemplate];
pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentHarnessEvent>;
pub async fn wait_for_idle(&self);
pub async fn wait_for_settled(&self);  // 所有 pending session writes 落盘
```

**实现要点（架构修正）：** Harness **不** 包装 Agent；它直接调用 `agent_loop()`，自己维护事件处理、状态机、session 写入。这与 TS 版的 AgentHarness 定位一致——是 Agent 的超集替代而非装饰。Agent 类作为不需要 session/skills 的简化使用场景独立存在。

**Pending session writes：** Harness 运行 turn 期间，配置变更（set_model 等）和消息记录通过 `pending_session_writes` 缓冲；turn 结束的 save point 一次性 flush，避免 turn 进行中频繁 IO 与 session 状态闪烁。

**`next_turn` 注入机制（无独立 channel）：** `harness.next_turn(text)` 不通过 channel，而是直接将消息追加到 `HarnessState.queued_next_turn`。下一次 `prompt()` 调用时：

```rust
async fn prompt(&self, text: String) -> Result<()> {
    // 1. 检查 Idle 阶段
    // 2. 取出 queued_next_turn 缓冲
    let queued = {
        let mut st = self.state.lock().unwrap();
        if st.phase != HarnessPhase::Idle { return Err(NotIdle); }
        std::mem::take(&mut st.queued_next_turn)
    };
    // 3. 合并：queued ++ [user(text)]
    let initial = queued.into_iter().chain(once(make_user(text))).collect();
    self.run_loop(initial).await
}
```

**事件处理管道（伪代码）：**

```rust
async fn run_loop(&self, initial_messages: Vec<AgentMessage>) -> Result<()> {
    // 1. 设置 Phase = Turning
    self.set_phase(HarnessPhase::Turning);

    // 2. 从 session 重建上下文，与 initial_messages 合并
    let built = self.session.build_context().await?;
    let ctx = AgentContext {
        system_prompt: self.compose_system_prompt(),  // 含 skills
        messages: built.messages.into_iter().chain(initial_messages).collect(),
    };

    // 3. 构造 LoopConfig（从 HarnessHooks + state）
    let cfg = self.build_loop_config();
    // 关键：在 build_loop_config 中：
    //  - 用 HookedTool 包装每个 tool 注入 before/after_tool_call
    //  - 复制其他 hook
    //  - 派生 steer_rx / follow_up_rx

    let mut stream = agent_loop(self.client.clone(), ctx, cfg);

    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TurnStart { index } => {
                self.emit(AgentHarnessEvent::Agent(event));
            }
            AgentEvent::MessageEnd { ref message, .. } => {
                // 把 assistant message 加入 pending writes
                self.push_pending_write(SessionEntryPayload::Message(
                    AgentMessage::Assistant(message.clone())
                ));
                self.emit(AgentHarnessEvent::Agent(event));
            }
            AgentEvent::ToolExecutionStart { ref tool_use_id, .. } => {
                self.state.lock().unwrap().pending_tool_calls.insert(tool_use_id.clone());
                self.emit(AgentHarnessEvent::Agent(event));
            }
            AgentEvent::ToolExecutionEnd { ref tool_use_id, ref result, .. } => {
                self.state.lock().unwrap().pending_tool_calls.remove(tool_use_id);
                // 把 tool result 加入 pending writes
                self.push_pending_write(SessionEntryPayload::Message(
                    AgentMessage::ToolResult(/* 从 result 构造 */)
                ));
                self.emit(AgentHarnessEvent::Agent(event));
            }
            AgentEvent::TurnEnd { .. } => {
                // Save point: flush pending session writes 到 storage
                let flushed = self.flush_pending_writes().await?;
                self.emit(AgentHarnessEvent::SavePoint { entries_flushed: flushed });
                self.emit(AgentHarnessEvent::Agent(event));
            }
            AgentEvent::AgentEnd { .. } => {
                self.emit(AgentHarnessEvent::Agent(event));
                self.emit(AgentHarnessEvent::Settled);
                break;
            }
            AgentEvent::Error(err) => {
                self.state.lock().unwrap().error_message = Some(err.to_string());
                self.emit(AgentHarnessEvent::Agent(AgentEvent::Error(err)));
            }
            other => {
                self.emit(AgentHarnessEvent::Agent(other));
            }
        }
    }

    // 4. 恢复 Phase = Idle
    self.set_phase(HarnessPhase::Idle);
    Ok(())
}
```

**`prepare_next_turn` 与 `set_active_tools` 的交互：** Harness 实现 `PrepareNextTurnHook` 的默认 wrapper——它在 hook 调用时读取最新 `HarnessState`（含 `active_tools`），并把 `NextTurnDirective.tools` / `active_tools` 字段填入。这是反向读取 HarnessState 的合法路径：wrapper 在闭包内持有 `Arc<Mutex<HarnessState>>`，仅在 hook 调用瞬间短锁读取——不跨 await。
