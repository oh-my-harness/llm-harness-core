# llm-harness-core 系统设计

**日期：** 2026-06-07
**状态：** 已批准（v2，已纳入审核意见）

## 1. 背景与目标

本项目是 [`@earendil-works/pi-agent-core`](../../../../pi-main/packages/agent) TypeScript 包的 Rust 全量重写。目标不是逐行翻译，而是学习其核心设计哲学，用 Rust 惯用法重新表达。

核心目标：
- 提供完整的 agent 运行时：低层循环、有状态 Agent、编排层 AgentHarness
- 全量对标 pi-agent-core 功能：会话持久化、上下文压缩、skills/templates、执行环境抽象
- 以 [`llm-api-adapter`](../../../../llm-api-adapter) 作为 LLM provider 层

## 2. Crate 结构

Cargo workspace，三个 crate：

```
llm-harness-core/
├── Cargo.toml               (workspace)
└── crates/
    ├── llm-harness-types/   (纯类型 + trait)
    ├── llm-harness-loop/    (core loop 引擎)
    └── llm-harness/         (Agent + Harness + Session + Compaction + Skills)
```

依赖关系：

```
llm-api-adapter  (外部 crate)
      │
      ▼
llm-harness-types  ←──────────────────┐
      │                               │
      ▼                               │
llm-harness-loop               llm-harness
      │                               │
      └───────────────►───────────────┘
```

## 3. `llm-harness-types`

**职责：** 零 IO 的纯类型层。所有跨 crate 共享的类型和 trait 均在此定义。

**依赖：** `serde`, `serde_json`, `futures`（仅 `BoxFuture`），`tokio-util`（`CancellationToken`），`thiserror`，`uuid`

**外部类型说明：**
- `LlmClient`：由 `llm-api-adapter` 提供的 trait，代表可发起流式 LLM 调用的客户端
- `CancellationToken`：来自 `tokio-util::sync`，用于跨任务取消传播

### 3.1 基础标识与错误类型

```rust
/// Session log 中每条 entry 的唯一标识，UUIDv7（时间有序）。
/// 兼任消息标识：消息即 SessionEntry::Message，其 EntryId 即消息 ID。
pub struct EntryId(pub uuid::Uuid);

/// Tool 执行失败。Tool 实现可用 `Other` 包装任意错误源。
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("tool aborted")]
    Aborted,
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Agent 在事件流中暴露的错误。
#[derive(thiserror::Error, Debug, Clone)]
pub enum AgentError {
    #[error("llm provider error: {0}")]
    Provider(String),
    #[error("tool error: {tool_name}: {message}")]
    Tool { tool_name: String, message: String },
    #[error("aborted")]
    Aborted,
    #[error("internal: {0}")]
    Internal(String),
}

/// LLM 自然停止原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    Other,
}
```

### 3.2 消息类型

`ContentBlock` 是 LLM 消息内容的最小单元，对应 Anthropic/OpenAI 的 content block 模型：

```rust
pub enum ContentBlock {
    Text  { text: String },
    Image { source: ImageSource },
    ToolUse { id: String, name: String, input: serde_json::Value },
}

/// 图片来源——预留 URL/外部引用扩展点，避免 base64 inline 锁死。
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    // 未来扩展：Url { url: String }, Id { id: String }
}
```

```rust
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    Custom(CustomMessage),       // 应用层自定义消息
}

pub struct UserMessage      { pub content: Vec<ContentBlock> }
pub struct AssistantMessage { pub content: Vec<ContentBlock>, pub stop_reason: Option<StopReason> }
pub struct ToolResultMessage { pub tool_use_id: String, pub content: Vec<ContentBlock>, pub is_error: bool }
pub struct CustomMessage    { pub r#type: String, pub data: serde_json::Value }
```

### 3.3 事件类型

Agent 的行为通过事件流暴露，调用方"观测"事件而非被回调驱动。

```rust
pub enum AgentEvent {
    TurnStart     { index: u32 },
    TurnEnd       { index: u32 },
    TextDelta     { text: String },
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, partial_input: String },
    ToolCallEnd   { id: String, result: Result<ToolResult, ToolError> },
    Error(AgentError),
    Done,
}
```

**事件传递语义（重要）：** Agent 层使用 `tokio::sync::broadcast` 分发事件。慢消费者超过 channel 容量后将丢失旧事件——这意味着：

- 调用方**不应**依赖事件流重建完整状态机；状态机应基于 Session log
- 事件流是"行为通知"，不是"真实来源"
- 容量在 `AgentOptions::event_channel_capacity` 中可配（默认 256）

### 3.4 Tool trait

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> &serde_json::Value;
    fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::Parallel }
    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>>;
}

pub enum ToolExecutionMode { Parallel, Sequential }

pub struct ToolContext {
    pub env: Arc<dyn ExecutionEnv>,
    pub abort: CancellationToken,
}
```

**生命周期注记：** `execute` 返回的 future 借用 `&self` 和 `&ToolContext`，不满足 `'static`。这是有意的：tool 实例可以持有非 Sync 内部状态而不需要 `Arc<Mutex<_>>` 包装；并行执行通过 `futures::future::join_all` 在 loop 任务内驱动（同任务并发，非跨任务并行）。如果未来发现需要真正的跨线程并行，再加一个 `'static` future 返回的并行 trait variant。

### 3.5 ExecutionEnv trait

TS 版将执行环境视为 Node.js 专属，在后期才抽象为 trait。本设计从第一天起即为 trait，支持 OS、WASM、测试 mock 等不同实现。

```rust
pub trait ExecutionEnv: Send + Sync {
    fn read_file<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<String, EnvError>>;
    fn write_file<'a>(&'a self, path: &'a Path, content: &'a str)
        -> BoxFuture<'a, Result<(), EnvError>>;
    fn list_dir<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Vec<FileInfo>, EnvError>>;
    fn execute_shell<'a>(&'a self, cmd: &'a str, abort: CancellationToken)
        -> BoxFuture<'a, Result<ShellOutput, EnvError>>;
    fn working_dir(&self) -> &Path;
}

pub struct ShellOutput { pub stdout: String, pub stderr: String, pub exit_code: i32 }
pub struct FileInfo    { pub path: PathBuf, pub is_dir: bool, pub size: u64 }
```

**权限模型：** `ExecutionEnv` 不提供细粒度权限。权限边界由实现方控制——例如 `OsEnv` 可配置允许的工作目录、shell 命令白名单。`Tool` 通过 `ToolContext.env` 拿到环境，能力范围等同于该 env 实例的能力。需要更细粒度控制的调用方应包装一个受限 env 注入 ToolContext。

### 3.6 Hook traits

集中定义在 types 层，避免在 loop 与 harness 层重复声明。

```rust
/// 每次 LLM 调用前对上下文做转换（compaction 通过此 hook 接入）。
pub trait TransformContextHook: Send + Sync {
    fn transform<'a>(
        &'a self,
        ctx: AgentContext,
    ) -> BoxFuture<'a, Result<AgentContext, AgentError>>;
}

pub struct BeforeToolCallCtx<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub args: &'a serde_json::Value,
}

pub enum BeforeToolCallDecision {
    /// 允许执行
    Allow,
    /// 替换 args 后执行
    Modify(serde_json::Value),
    /// 拒绝执行，返回结果
    Deny(ToolResult),
}

pub trait BeforeToolCallHook: Send + Sync {
    fn on_call<'a>(&'a self, ctx: BeforeToolCallCtx<'a>)
        -> BoxFuture<'a, BeforeToolCallDecision>;
}

pub struct AfterToolCallCtx<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub args: &'a serde_json::Value,
    pub result: &'a Result<ToolResult, ToolError>,
}

pub trait AfterToolCallHook: Send + Sync {
    fn on_complete<'a>(&'a self, ctx: AfterToolCallCtx<'a>) -> BoxFuture<'a, ()>;
}

pub struct ShouldStopCtx<'a> {
    pub last_assistant: &'a AssistantMessage,
    pub stop_reason: StopReason,
    pub turn_index: u32,
}

pub trait ShouldStopHook: Send + Sync {
    /// 仅在 LLM 自然停止时调用。返回 true 才停止；返回 false 强制再跑一轮。
    /// 不能用于强制中断进行中的 turn——中断走 abort()。
    fn should_stop<'a>(&'a self, ctx: ShouldStopCtx<'a>) -> BoxFuture<'a, bool>;
}

/// Harness 专属的 turn 边界 hook，包含 session 上下文。
pub struct BeforeTurnCtx<'a> {
    pub turn_index: u32,
    pub snapshot: &'a TurnSnapshot,
}
pub trait BeforeTurnHook: Send + Sync {
    fn before_turn<'a>(&'a self, ctx: BeforeTurnCtx<'a>) -> BoxFuture<'a, ()>;
}

pub struct AfterTurnCtx<'a> {
    pub turn_index: u32,
    pub new_messages: &'a [AgentMessage],
}
pub trait AfterTurnHook: Send + Sync {
    fn after_turn<'a>(&'a self, ctx: AfterTurnCtx<'a>) -> BoxFuture<'a, ()>;
}
```

### 3.7 其他基础类型

```rust
pub enum ThinkingLevel { Off, Minimal, Low, Medium, High, XHigh }

pub struct AgentContext {
    pub system_prompt: Option<String>,
    pub messages: Vec<AgentMessage>,
}

pub struct ToolResult {
    pub content: Vec<ContentBlock>,
    pub details: serde_json::Value,
}

/// Turn 快照：在 types 层定义以便 hook 引用。
#[derive(Clone)]
pub struct TurnSnapshot {
    pub model:          String,
    pub thinking_level: ThinkingLevel,
    pub tools:          Vec<Arc<dyn Tool>>,
    pub system_prompt:  Option<String>,
}
```

## 4. `llm-harness-loop`

**职责：** 纯函数式 agent loop——给定上下文与配置，返回事件流。不持有持久状态，不管理会话。

**依赖：** `llm-harness-types`, `llm-api-adapter`, `tokio`, `tokio-stream`, `futures`

### 4.1 API

```rust
pub struct LoopConfig {
    pub tools:             Vec<Arc<dyn Tool>>,
    pub default_execution_mode: ToolExecutionMode,

    /// 每次 LLM 调用前转换上下文（compaction 通过此钩子挂入，与 loop 解耦）。
    pub transform_context: Option<Arc<dyn TransformContextHook>>,

    /// 响应式注入：turn 边界注入消息（steer），agent 停止后注入（follow_up）。
    pub steer_rx:     Option<mpsc::Receiver<String>>,
    pub follow_up_rx: Option<mpsc::Receiver<String>>,

    /// LLM 自然停止时调用方决定是否继续。
    pub should_stop:  Option<Arc<dyn ShouldStopHook>>,
}

pub fn agent_loop(
    client: Arc<dyn LlmClient>,
    ctx:    AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send
```

**注：** Tool call 拦截 hook（`BeforeToolCallHook`、`AfterToolCallHook`）仅在 Harness 层挂入；Loop 层不接受这两个 hook，以消除两层重复。Loop 是纯函数式底层，调用方如需 hook 应使用 Harness。

### 4.2 Tool batch 执行：分治调度

每轮 LLM 返回的 tool call 集合按 LLM 返回顺序，以 Sequential tool 为分割点切分子组：

```
LLM 返回: [P1, P2, S1, P3, P4, S2, P5]
执行:
  子组1: join_all(P1, P2)         ← 并发
  子组2: 单独执行 S1               ← 顺序
  子组3: join_all(P3, P4)         ← 并发
  子组4: 单独执行 S2               ← 顺序
  子组5: 执行 P5                   ← 单调用，自然顺序
```

- 默认 mode 由 `LoopConfig.default_execution_mode` 提供，tool 自身 `execution_mode()` 覆盖默认
- 并发组内任一 tool 失败不影响同组其他 tool（结果原样返回给 LLM）
- 子组间严格顺序：上一组所有 tool 完成后才开始下一组

这避免了"一个 Sequential 让整批退化"的连坐问题，同时保留了顺序语义。

### 4.3 Steering vs Follow-up（队列语义）

- **Steer**：在一轮 tool 执行完成、下一次 LLM 调用之前，将 channel 中**所有**待处理消息**全部**作为 user 消息注入。多条 steer 按 FIFO 全部生效。
- **Follow-up**：在 agent 自然停止（`should_stop` 返回 true 或无更多 tool call）后，从 channel 取**一条**消息触发新一轮。剩余消息保留在 channel 中等待下一次停止时取出。

两者均通过 `mpsc::Receiver` 实现。

### 4.4 should_stop 优先级（明确规则）

```
LLM stop_reason = ToolUse:
  → 不询问 should_stop，直接执行 tool batch 然后下一轮（LLM 主导继续）

LLM stop_reason ∈ {EndTurn, MaxTokens, StopSequence, Other}:
  → 询问 should_stop（如配置）：
      返回 true:  停止 loop，进入 follow_up 检查
      返回 false: 强制再跑一轮（注入空 user 消息触发？由实现决定提示策略）
  → 未配置 should_stop：停止 loop

abort 信号（来自 CancellationToken）：
  → 任何时候立即终止，发出 AgentEvent::Error(AgentError::Aborted) + Done
```

`should_stop` 仅在 LLM 已自然停止时被询问"是否继续"，不能强制中断进行中的 turn——中断走 `abort()`。

### 4.5 事件传递

TS 版 `Agent` 类需要 await 所有 `agent_end` 监听器（因 JS 事件监听器默认 fire-and-forget）。Rust 的 `Stream` 是拉取式，loop 返回的是 cold stream——只有被 poll 才推进。调用方决定何时消费即决定何时推进。框架层无需做额外结算保证。

## 5. `llm-harness`

**职责：** 在 loop 之上提供有状态 Agent、编排层 AgentHarness、会话持久化、上下文压缩、skills/templates 加载。

**依赖：** `llm-harness-types`, `llm-harness-loop`, `tokio`, `serde_json`, `serde_yaml`（frontmatter）

### 5.1 Agent

有状态包装器，在 `agent_loop` Stream 之上提供订阅、阶段控制、响应式注入。

```rust
pub struct Agent {
    client:       Arc<dyn LlmClient>,
    state:        Arc<std::sync::Mutex<AgentState>>,
    event_tx:     tokio::sync::broadcast::Sender<AgentEvent>,
    steer_tx:     tokio::sync::mpsc::Sender<String>,
    follow_up_tx: tokio::sync::mpsc::Sender<String>,
    abort:        CancellationToken,
}

struct AgentState {
    phase:          AgentPhase,
    model:          String,
    thinking_level: ThinkingLevel,
    tools:          Vec<Arc<dyn Tool>>,
    messages:       Vec<AgentMessage>,
    system_prompt:  Option<String>,
}

#[derive(PartialEq)]
enum AgentPhase { Idle, Running }
```

**锁使用约束（严格）：** `std::sync::Mutex` **绝不**跨越 `.await`。统一模式：

```rust
// ✅ 正确：lock → 快照 → drop → await
let snapshot = {
    let st = self.state.lock().unwrap();
    TurnSnapshot {
        model: st.model.clone(),
        thinking_level: st.thinking_level,
        tools: st.tools.clone(),
        system_prompt: st.system_prompt.clone(),
    }
}; // 锁在此释放
let stream = agent_loop(client, build_ctx(&snapshot, ...), cfg); // await 期间无锁
```

锁仅保护快速的同步操作（读取/写入字段、推送新消息到 `messages`）。任何跨 await 的操作必须先取出数据副本。

方法按阶段分类：

```rust
// 结构性操作：仅 Idle 阶段可调用，否则返回 Err(AgentError::NotIdle)
pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentError>;

// 队列操作：任何阶段均安全
pub fn steer(&self, text: impl Into<String>);   // 直接 try_send 到 channel
pub fn follow_up(&self, text: impl Into<String>);
pub fn abort(&self);                            // 触发 CancellationToken

// 观测
pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent>;
pub async fn wait_for_idle(&self);
```

**并发 prompt 行为：** 两个调用方同时在 Idle 状态下调用 `prompt()`，只有先获取 state 锁的一方能将 phase 转为 Running 并继续；另一方读到 Running，返回 `Err(NotIdle)`。Agent 不内置队列，调用方如需排队应在外层包装。

### 5.2 Turn 快照

**设计哲学（来自 pi-agent-core）：** 每次 turn 开始时，将当前 model、tools、系统提示做一次不可变快照（`TurnSnapshot`，定义见 §3.7）。turn 进行中对这些配置的修改不影响当前 turn，仅影响下一轮。

这同时是 §5.1 锁使用约束的天然实现——快照本身就是"跨 await 前 clone 出的副本"。

### 5.3 Session

**设计哲学：** 会话是一条**不可变的类型化追加日志**，而非 messages 数组。支持回放、审计；compaction 不删除历史，而是追加一条 Compaction entry。

```rust
pub enum SessionEntry {
    Message(MessageEntry),
    ModelChange         { to: String },
    ThinkingLevelChange { to: ThinkingLevel },
    ToolsChange         { active: Vec<String> },
    Compaction(CompactionEntry),       // 压缩摘要替换旧消息范围
    Label               { name: String }, // 给当前位置命名，便于导航

    // 未来分支扩展点（v1 不实现）：
    // BranchPoint { from: EntryId },
    // BranchSwitch { to: EntryId },
}

pub struct CompactionEntry {
    pub summary_message: AgentMessage,         // 注入到上下文中的摘要消息
    pub compressed_range: (EntryId, EntryId),  // 被替换的连续 entry 范围（闭区间）
}

/// 底层存储 trait：只负责字节追加与原样读取，不解释 entry 语义。
pub trait SessionStorage: Send + Sync {
    fn append<'a>(&'a self, entry: SessionEntry)
        -> BoxFuture<'a, Result<EntryId, SessionError>>;
    fn read_range_raw<'a>(&'a self, from: Option<EntryId>)
        -> BoxFuture<'a, Result<Vec<(EntryId, SessionEntry)>, SessionError>>;
}

/// 高层 Session 接口：解释 Compaction，提供"当前有效上下文"视图。
/// Session 是 SessionStorage 上的包装层，非 trait——所有 storage 实现共享同一段逻辑。
pub struct Session {
    storage: Arc<dyn SessionStorage>,
}

impl Session {
    /// 原始读取，所有 entry 原样返回（用于审计、调试、分支导航）。
    pub async fn read_raw(&self, from: Option<EntryId>)
        -> Result<Vec<(EntryId, SessionEntry)>, SessionError>;

    /// 有效读取：跳过被 Compaction 覆盖的历史 entry，
    /// 将 Compaction 的 summary_message 插入对应位置。
    /// 返回的消息序列可直接构造 AgentContext。
    pub async fn read_effective(&self) -> Result<Vec<AgentMessage>, SessionError>;

    pub async fn append(&self, entry: SessionEntry) -> Result<EntryId, SessionError>;
}

pub struct JsonlSessionStorage   { path: PathBuf }
pub struct InMemorySessionStorage {
    entries: Arc<std::sync::Mutex<Vec<(EntryId, SessionEntry)>>>,
}
```

**v1 范围说明：** 分支与 `Leaf`/`BranchSummary` entry 从 v1 范围中**移除**，避免空洞设计。Session 当前为线性追加日志。`SessionEntry` enum 预留 `BranchPoint` / `BranchSwitch` 注释，明示这是 v1.x 扩展方向。

**超大 entry 处理：** v1 不引入外部存储引用。单条 entry 在 JSONL 中即为单行，超大 tool result（如完整 shell 输出）应由调用方在 tool 实现中预先截断（参考 pi-agent-core 的 `truncateOutput` 模式）。Session 层只保证字节追加正确，不对单条大小设硬上限。

### 5.4 Compaction

```rust
pub struct CompactionSettings {
    pub token_threshold: usize,  // 超过此 token 数触发压缩
    pub summary_model:   String, // 用便宜模型做摘要
    pub retain_recent:   usize,  // 保留最近 N 条消息不压缩
}

/// 独立 LLM 调用——使用 summary_model，不复用 Agent 的主模型客户端。
/// Compaction 由 Harness 在 Idle 阶段同步触发，期间 Harness 进入 Compacting 阶段。
pub async fn compact(
    client:   &dyn LlmClient,
    messages: &[AgentMessage],
    settings: &CompactionSettings,
) -> Result<CompactionResult, CompactionError>;

pub struct CompactionResult {
    pub retained:        Vec<AgentMessage>, // 保留的消息（含 summary 消息）
    pub summary_message: AgentMessage,
    pub compressed_range: (EntryId, EntryId), // 写入 Session::CompactionEntry
}
```

**LLM 调用归属（明确）：** compaction 的 LLM 调用是**独立的**，由 Harness 直接调用 `LlmClient`，不经过 Agent。Agent 主循环不感知 compaction 在运行。

**阶段交互（明确）：**
- v1：`harness.compact()` 仅在 Harness 处于 Idle 时可调用。compact 期间 Harness 转为 `Compacting`，期间 `prompt()` 等结构性操作返回 `Err(NotIdle)`。这避免 compact 进行中上下文还在变化。
- v1.x 可考虑后台压缩（Idle 期间异步压缩，对用户不可见），但需要小心 compact 结束时若 Agent 已开始 prompt，要决定丢弃还是合并。

### 5.5 Skills 与 PromptTemplates

```rust
pub struct Skill {
    pub name:        String,
    pub description: String,
    pub content:     String,
    pub source:      PathBuf,
}

pub struct PromptTemplate {
    pub name:    String,
    pub content: String,  // 含 {{placeholder}} 占位符
    pub source:  PathBuf,
}

pub struct SkillDiagnostic {
    pub source:  PathBuf,
    pub level:   DiagnosticLevel,  // Warn | Error
    pub message: String,
}

// 递归扫描目录，解析 YAML frontmatter，遵守 .gitignore 规则
pub async fn load_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<Skill>, Vec<SkillDiagnostic>);

pub fn format_skill_for_system_prompt(skills: &[Skill]) -> String;
pub fn invoke_template(
    template: &PromptTemplate,
    args:     &HashMap<String, String>,
) -> Result<String, TemplateError>;
```

### 5.6 AgentHarness

编排层：将 Agent、Session、Compaction、Skills 组合，并管理运行阶段。

```rust
pub struct AgentHarness {
    agent:     Agent,
    session:   Session,
    env:       Arc<dyn ExecutionEnv>,
    skills:    Vec<Skill>,
    templates: Vec<PromptTemplate>,
    phase:     std::sync::Mutex<HarnessPhase>,
    hooks:     HarnessHooks,
}

#[derive(PartialEq, Clone, Copy)]
enum HarnessPhase { Idle, Turning, Compacting }

/// 所有 hook 仅在 Harness 层定义，loop 层不接受 tool call hook。
pub struct HarnessHooks {
    pub before_turn:      Option<Arc<dyn BeforeTurnHook>>,
    pub after_turn:       Option<Arc<dyn AfterTurnHook>>,
    pub before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    pub after_tool_call:  Option<Arc<dyn AfterToolCallHook>>,
    pub transform_context: Option<Arc<dyn TransformContextHook>>,
    pub should_stop:      Option<Arc<dyn ShouldStopHook>>,
}
```

**hook 流转：** Harness 将 `transform_context` / `should_stop` 直接传给 `LoopConfig`；`before_tool_call` / `after_tool_call` 由 Harness 通过包装一个内部 `TransformContextHook` 或拦截 tool 调用实现——具体机制：Harness 在构造 `LoopConfig.tools` 时将每个 `Arc<dyn Tool>` 包装成 `HookedTool`，由 `HookedTool` 在 `execute()` 内调用 hook。这样 loop 保持纯净，hook 集中归 Harness。

**Harness 事件：**

```rust
pub enum AgentHarnessEvent {
    /// 来自底层 Agent 的事件透传
    Agent(AgentEvent),
    /// Harness 自身阶段变更
    PhaseChange { from: HarnessPhase, to: HarnessPhase },
    /// Compaction 生命周期
    CompactionStart,
    CompactionEnd { result: Result<CompactionStats, String> },
    /// Skills / templates 重新加载
    SkillsReloaded { count: usize, diagnostics: Vec<SkillDiagnostic> },
}

pub struct CompactionStats {
    pub before_tokens: usize,
    pub after_tokens:  usize,
    pub compressed:    usize,  // 被压缩的 entry 数
}
```

操作分类：

```rust
// 结构性操作：仅 Idle 阶段，否则 Err(HarnessError::NotIdle)
pub async fn prompt(&self, text: impl Into<String>) -> Result<(), HarnessError>;
pub async fn compact(&self) -> Result<CompactionStats, HarnessError>;
pub async fn reload_skills(&self) -> Result<(), HarnessError>;
pub async fn prompt_from_template(
    &self, name: &str, args: HashMap<String, String>,
) -> Result<(), HarnessError>;

// 队列操作：任何阶段均安全
pub fn steer(&self, text: impl Into<String>);
pub fn follow_up(&self, text: impl Into<String>);
pub fn abort(&self);

// 观测
pub fn subscribe(&self) -> impl Stream<Item = AgentHarnessEvent>;
```

## 6. 关键设计决策汇总

| 决策 | 选择 | 理由 |
|---|---|---|
| 事件流机制 | `Stream` + `broadcast::Receiver` | Rust 拉取式，无需框架层结算；明示慢消费者会丢事件 |
| 工具定义 | `dyn Tool` trait object | 调用方可自由实现，无需依赖 typebox |
| Tool 执行调度 | 分治：按 Sequential 切分子组，组内并发 | 避免连坐降级，保留顺序语义 |
| 执行环境 | `dyn ExecutionEnv` trait，第一天即抽象 | 支持 OS/WASM/mock，优于 TS 版后期抽象 |
| 锁策略 | `std::sync::Mutex` + 强制快照模式 | 性能最佳；约束清晰可审计；杜绝跨 await 持锁 |
| 阶段锁 | 运行时枚举 `HarnessPhase` | typestate 在 async + 长生命周期场景下过于繁琐 |
| 会话模型 | 追加日志 + `SessionEntry` enum | 不可变、可压缩溯源；v1 单分支，分支推迟 |
| Session 读取 | 双层接口：`read_raw` + `read_effective` | 底层透明追加，高层解释 Compaction |
| Compaction LLM 调用 | 独立调用 `summary_model`，不复用 Agent | 解耦；v1 同步触发，Agent 必须 Idle |
| Tool call hooks | 仅 Harness 层定义 | 消除两层重复 |
| `should_stop` 语义 | 仅在自然停止时询问；不能强制中断 | 中断走 `abort()` 一条路径 |
| Steer 语义 | FIFO 全部注入 | 多条 steer 都生效 |
| 图片引用 | `ImageSource` enum 预留 URL/Id | 避免 base64 锁死 |
| Tool 参数 | `serde_json::Value`（JSON Schema） | 灵活，后期可引入 typed 泛型工具 |
| Crate 拆分 | workspace + 3 crates | 关注点分离，允许按层依赖 |

## 7. 不在范围内（v1）

- **Proxy 模式**（browser → backend 流式转发）：暂不实现，可后续作为独立 feature
- **WASM 目标**：`ExecutionEnv` trait 已预留扩展点，WASM 实现留待后期
- **Session 分支**：v1 仅线性追加日志；分支模型推迟到 v1.x，届时定义存储语义后再实现
- **后台 compaction**：v1 同步触发；v1.x 可考虑异步压缩
- **细粒度权限模型**：v1 由 `ExecutionEnv` 实现方控制；capability token 机制留待后期
- **超大 entry 外部存储**：v1 调用方负责截断；外部 blob 引用留待后期
- **Agent loop 以外的 agent 框架能力**（规划、记忆管理）：属于调用方责任
