### 5.6 AgentHarness

编排层：直接驱动 `agent_loop()`（不内嵌 Agent），自己管理 session 写入、配置变更记录、resource 解析。

> **为什么 AgentHarness 不包装 Agent？** TS 版的关键架构决策——AgentHarness 是 Agent 的**超集替代**而非装饰器。包装模式的问题：(1) Agent 的 messages 数组和 Session 的 entry log 冗余存储同一份数据——需要同步；(2) Agent 的配置状态（model/tools）和 Session 的配置记录需要双向同步；(3) Agent 的 `prompt()` 和 Harness 的 `prompt()` 有两套并发控制。直接驱动 `agent_loop()` 消除了这些同步问题——Harness 是状态和行为的唯一拥有者。
>
> **Agent 类依然存在的原因：** 不需要 session/skills 的简化场景——如脚本化的 LLM 调用、测试、原型开发。Agent 不依赖 Session/Skills/Templates——依赖更少，设置更简单。

---

#### 结构体定义

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
```

> **字段设计理由（与 Agent 的对比）：**
>
> **AgentHarness 独有字段：** `session`（持久化）、`env`（执行环境）、`skills`/`templates`（资源）、`hooks`（钩子集合）、`stream_options`（传输配置）、`auth`（认证）。
>
> **AgentHarness 与 Agent 共有字段：** `client`、`state`、`event_tx`、`steer_tx`/`follow_up_tx`、`abort`。命名和用途一致。
>
> **`convert_to_llm` 独立于 `HarnessHooks`：** 它是 loop crate 定义的 trait——语义上不属于 HarnessHook（它是必需的数据转换，而非可选的行为钩子）。独立存储避免在 HarnessHooks 中引入 loop crate 的类型依赖。
>
> **内部 channels 外部不可见：** `steer_tx`/`follow_up_tx` 的 receiver 端在每次启动 loop 时派生并传给 `LoopConfig`。调用方不直接接触 channels——他们通过 `harness.steer(text)` 等方法间接使用。

---

```rust
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
```

> **HarnessState 与 AgentState 的差异：**
>
> **HarnessState 独有：**
> - `active_tools: Option<HashSet<String>>`——工具的子集激活控制。`None` 表示所有已注册工具都激活。这允许 "注册 10 个工具，但当前对话只用其中 3 个"。
> - `pending_session_writes: Vec<SessionEntryPayload>`——运行期间延迟写入缓冲。避免每个消息都立即写 session（频繁 IO），改为 turn 结束时批量 flush。
> - `queued_next_turn: Vec<AgentMessage>`——`next_turn()` 消息的缓冲。与 steer/follow-up 不同——next_turn 消息不在当前 turn 注入，而在下一次 `prompt()` 时注入。
>
> **AgentState 独有（HarnessState 没有）：**
> - `messages: Vec<AgentMessage>`——Agent 自己管理消息历史。Harness 从 session 读取历史。
> - `error_message: Option<String>`——Agent 的独立错误追踪。Harness 通过事件的 error 处理来追踪，不需要独立字段。
>
> **共享字段：** `phase`、`model`、`model_info`、`thinking_level`、`tools`、`system_prompt`、`streaming_message`、`pending_tool_calls`。

---

```rust
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

> **HarnessHooks 的设计原则：**
> - **全可选**——不设置 hook 时 Harness 使用默认行为。
> - **全 `Arc<dyn Trait>`**——允许 hook 实例在多个 Harness 之间共享（如全局的审计 hook）。
> - **唯一真相源**——Harness 的 hook 配置只存在于 HarnessHooks 中。LoopConfig 由 Harness 内部临时构造，调用方不接触 LoopConfig。
> - **`convert_to_llm` 不在 HarnessHooks 中**——它在 AgentHarness struct 上作为独立字段。原因是 (1) 它是必需的（非可选），(2) 它来自 loop crate，类型系统上与其他 hook 不同。

---

#### Harness 事件

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

> **AgentHarnessEvent 的设计理由：**
>
> **`Agent(AgentEvent)` 作为包装变体：** Harness 透传所有 Agent 事件给订阅者——订阅 Harness 的调用方可以只监听一个事件流，从中提取 AgentEvent（用于 UI 渲染）和 HarnessEvent（用于状态同步）。
>
> **配置变更事件（ModelUpdate / ThinkingLevelUpdate / ToolsUpdate / ActiveToolsUpdate / ResourcesUpdate / SessionInfoUpdate）：** 携带变更前后的值——UI 可以用 `from`/`to` 做动画过渡，或记录变更日志。这些事件在 `set_model()` 等方法成功执行后发出。
>
> **Compaction 事件（CompactionStart / CompactionEnd）：** 异步操作的开始和结束通知。`CompactionEnd` 携带 `stats`（成功时）或 `error`（失败时）——调用方可以据此更新 UI。
>
> **队列事件（QueueUpdate）：** steer 和 follow-up 队列长度变更时发出。UI 可以显示 "有 2 条待注入的 steer 消息"。
>
> **SavePoint：** turn 结束时发出，告知 pending writes 已全部落盘。`entries_flushed` 是本次 flush 的 entry 数量。
>
> **分支事件（BranchForked / BranchSwitched / BranchDeleted / BranchSummarized）：** 分支生命周期通知。UI 可以据此刷新分支列表或显示通知。
>
> **Aborted / Settled：** AgentHarness 的生命周期边界——`Aborted` 在 `abort()` 完成后发出，`Settled` 在一次完整运行（prompt → AgentEnd）结束后发出。
>
> **为什么归并为 17 种变体而非 TS 的 20+？** TS 为每个 hook 点提供了独立事件类型（`before_agent_start`、`context`、`before_provider_request`...），总共 20+。Rust 的 Harness 事件更聚焦于 "状态变更通知" 而非 "hook 调用点"。调用方需要响应状态变更——模型变了、队列变了、分支变了。Hook 调用点是框架内部流程——调用方通过实现 trait 参与，不需要事件通知。

---

#### 操作 API

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
pub async fn fork_branch(&self, from_entry: EntryId, label: Option<String>)
    -> Result<EntryId, HarnessError>;
pub async fn navigate_tree(&self, target: EntryId) -> Result<(), HarnessError>;
pub async fn list_branches(&self) -> Result<Vec<BranchInfo>, HarnessError>;
pub async fn delete_branch(&self, leaf: EntryId) -> Result<(), HarnessError>;
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

> **方法分类的设计理由：**
>
> **结构性操作（仅 Idle）：**
> - `prompt` / `prompt_with_messages`：启动一次 agent 运行。`text` 版本是最常用场景——用户输入文本。
> - `prompt_from_template`：加载模板，用 `args` 填充，作为 prompt 发送。等价于 `prompt(invoke_template(template, args))`。
> - `skill(name, additional)`：显式调用 skill。内部查找 skill，调用 `format_skill_invocation`，作为 prompt 发送。这是 TS `harness.skill()` 的等价物。
> - `compact()`：触发 compaction。仅在 Idle 时可用——防止在 agent 运行期间并发修改 session。
> - `reload_resources`：重新扫描 skill/template 目录，更新内部列表。用于 "skill 文件被外部修改后刷新"。
>
> **运行时配置（任何阶段）：**
> - `set_model` / `set_thinking_level` / `set_tools` / `set_active_tools`——变更立即写入 HarnessState（通过锁）。如果处于 Running 阶段，还会通过 `pending_session_writes` 缓冲——turn 结束时 flush 到 session log。如果处于 Idle 阶段，直接写入 session。
> - `set_session_name`——修改会话名称，写入 `SessionInfo` entry。
> - **为什么配置方法返回 `Result`？** 写入 session 可能因为 I/O 错误而失败（Idle 阶段直接写 session）。
>
> **分支操作（仅 Idle）：**
> - `fork_branch(from_entry, label)`——在当前 session 内从历史 entry 创建新分支。等价于 Session 的 `fork_branch` + 事件发出。
> - `navigate_tree(target)`——切换到另一条分支。等价于 Session 的 `navigate_to` + 事件发出 + 上下文重建。
> - `list_branches`——获取所有分支的列表。UI 用于分支选择器。
> - `delete_branch(leaf)`——删除分支。等同于 Session 的 `delete_branch` + 事件发出。
> - `generate_branch_summary(leaf)`——调用 LLM 为指定分支生成摘要，写入 `BranchSummary` entry。这是可能较慢的操作（需要 LLM 调用）。
>
> **队列操作（任何阶段安全）：** 与 Agent 的队列操作语义相同——文本便捷方法 + 清空/查询方法。额外提供 `next_turn(text)`——与 steer/follow-up 不同，next_turn 消息不在当前 turn 注入。
>
> **观测方法：** `skills()` 和 `templates()` 返回引用（不 clone）——调用方可以读取但不能修改（内部 Vec 通过 `&self` 共享）。

---

#### 实现要点

**实现要点（架构修正）：** Harness **不** 包装 Agent；它直接调用 `agent_loop()`，自己维护事件处理、状态机、session 写入。这与 TS 版的 AgentHarness 定位一致——是 Agent 的超集替代而非装饰。Agent 类作为不需要 session/skills 的简化使用场景独立存在。

**Pending session writes：** Harness 运行 turn 期间，配置变更（set_model 等）和消息记录通过 `pending_session_writes` 缓冲；turn 结束的 save point 一次性 flush，避免 turn 进行中频繁 IO 与 session 状态闪烁。

> **Pending writes 的设计理由：** (1) 减少 I/O 次数——一个 turn 可能产生多条消息（assistant message + 多个 tool result）和配置变更，一次 flush 更高效。(2) 原子性——turn 期间的 session 变更在 save point 之前不可见（对其他 reader），如果 turn 失败，pending writes 可以直接丢弃而不污染 session。(3) 避免 "半完成 turn" 的 session 状态——如果 agent 在 tool 执行到一半时崩溃，session log 不会包含不完整的 tool 结果。

---

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

> **为什么 next_turn 不用 channel？** next_turn 是 "在当前 turn 结束后、下一次 prompt 时" 注入——它不通过 loop 内部的 channel poll，而是由 Harness 在构造初始消息时手动合并。这与 steer/follow-up 不同——steer/follow-up 需要在 loop 运行期间实时注入，必须通过 channel。next_turn 的注入时机在 loop 启动之前，不需要 channel 的实时性。
>
> **为什么合并到初始消息之前？** TS 的 `nextTurn` 行为——queued next_turn 消息排在用户的新 prompt 之前。这使得调用方可以在上一条消息到达后、下一条消息发送前注入上下文（如 "在上一轮中你修改了文件 X，现在请测试它"）。

---

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

> **事件处理管道的设计要点：**
>
> **MessageEnd → push_pending_write：** assistant 消息在 `MessageEnd` 时（不是 `MessageStart`）加入 pending writes——此时消息已经完整（含 stop_reason 和 usage）。如果 LLM 调用失败（中途 Error），`MessageEnd` 可能不会到达——对应地，pending writes 中不会有不完整的消息。
>
> **ToolExecutionStart/End → pending_tool_calls 更新：** 同步维护 `pending_tool_calls` 集合——UI 可以轮询此集合显示 "正在执行 read_file, grep..."。注意 `pending_tool_calls` 更新在 `pending_session_writes` 之前——因为前者是即时状态，后者是延迟持久化。
>
> **ToolExecutionEnd → push_pending_write：** tool result 在 execution 结束后写入 pending。注意这里从 `result: Result<ToolResult, ToolError>` 构造 `ToolResultMessage`——成功和失败都作为 tool result message 发送给 LLM。
>
> **TurnEnd → SavePoint：** turn 结束时 flush 所有 pending writes。这是原子提交点——turn 中的所有消息和配置变更一次性写入 session。如果 flush 失败（I/O 错误），返回 `Err` 并终止 run_loop。
>
> **AgentEnd → Settled + break：** `AgentEnd` 携带 `new_messages`——这是 loop 提供的完整消息列表。AgentHarness 不直接使用 `new_messages`（因为已经通过 `MessageEnd`/`ToolExecutionEnd` 逐个处理了），但 `AgentEnd` 到达是 "loop 确认完成" 的信号。此时发出 `Settled` 事件，退出循环。
>
> **Error 处理：** loop 在不可恢复错误时发出 `Error`。Harness 在 state 中记录错误信息（用于后续诊断），透传事件给订阅者。注意 Error 之后通常紧接着 `AgentEnd`——loop 在发出 Error 后立即终止。

---

**`prepare_next_turn` 与 `set_active_tools` 的交互：** Harness 实现 `PrepareNextTurnHook` 的默认 wrapper——它在 hook 调用时读取最新 `HarnessState`（含 `active_tools`），并把 `NextTurnDirective.tools` / `active_tools` 字段填入。这是反向读取 HarnessState 的合法路径：wrapper 在闭包内持有 `Arc<Mutex<HarnessState>>`，仅在 hook 调用瞬间短锁读取——不跨 await。

> **为什么 PrepareNextTurnHook 需要访问 HarnessState？** hook 需要知道 "当前 Harness 的配置是什么" 才能返回正确的 `NextTurnDirective`。例如，如果用户在 turn 之间调用了 `set_active_tools(["read_file", "grep"])`，prepare_next_turn hook 需要知道这个变化，并让 loop 在下一轮使用新的工具列表。
>
> **"反向引用"的安全实现：** Harness 提供的默认 prepare_next_turn wrapper 在闭包中持有 `Arc<Mutex<HarnessState>>`——这是 Harness 自己持有的同一个 Arc。当 loop 调用 hook 时，wrapper 短暂持锁读取需要的字段（`active_tools`、`model`、`thinking_level`），立即释放锁，然后返回 directive。不跨 await 持锁——安全。
>
> **用户提供的 hook 与默认 wrapper 的链式：** 如果用户在 `HarnessHooks.prepare_next_turn` 中提供了自定义 hook，Harness 会在默认 wrapper **之后**调用它——用户的 hook 可以进一步覆盖默认 directive。这允许用户拦截并修改 "从 session 恢复的配置"。
