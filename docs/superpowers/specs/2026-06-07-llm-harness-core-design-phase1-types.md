## 3. `llm-harness-types`

**职责：** 零 IO 的纯类型层。所有跨 crate 共享的类型和 trait 均在此定义。

**依赖：** `serde`, `serde_json`, `futures`（`BoxFuture`），`tokio`（feature = `sync`，用于 `mpsc::Sender`），`tokio-util`（`CancellationToken`），`thiserror`，`uuid`，`chrono`

**外部类型说明：**
- `LlmClient`：由 `llm-api-adapter` 提供的 trait，代表可发起流式 LLM 调用的客户端
- `ModelInfo`：由 `llm-api-adapter` 提供，含 `provider`、`api`、`model_id`、`context_window`、`max_tokens`、`cost` 等元数据；compaction token 估算依赖此结构
- `CancellationToken`：来自 `tokio-util::sync`，用于跨任务取消传播

### 3.1 基础标识与错误类型

```rust
/// Session log 中每条 entry 的唯一标识，UUIDv7（时间有序）。
/// 兼任消息标识：消息即 SessionEntry::Message，其 EntryId 即消息 ID。
/// 必须实现 Display/FromStr 以便 JSONL 序列化和跨进程引用。
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct EntryId(pub uuid::Uuid);
// impl Display, FromStr, Serialize, Deserialize (字符串形式)

/// Tool 执行失败。
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid arguments: {0}")] InvalidArguments(String),
    #[error("tool aborted")] Aborted,
    #[error("tool execution failed: {0}")] Execution(String),
    #[error(transparent)] Other(#[from] anyhow::Error),
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum AgentError {
    #[error("llm provider error: {0}")] Provider(String),
    #[error("tool error: {tool_name}: {message}")] Tool { tool_name: String, message: String },
    #[error("aborted")] Aborted,
    #[error("agent is not idle")] NotIdle,
    #[error("internal: {0}")] Internal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason { EndTurn, MaxTokens, StopSequence, ToolUse, Other }

/// 执行环境错误——对应 TS 的 8 种 FileSystem/Shell 错误码
#[derive(thiserror::Error, Debug)]
pub enum EnvError {
    #[error("path not found: {0}")]                    NotFound(PathBuf),
    #[error("permission denied: {0}")]                 PermissionDenied(PathBuf),
    #[error("path already exists: {0}")]               AlreadyExists(PathBuf),
    #[error("not a directory: {0}")]                   NotADirectory(PathBuf),
    #[error("is a directory: {0}")]                    IsADirectory(PathBuf),
    #[error("operation aborted")]                      Aborted,
    #[error("invalid utf-8 in {0}")]                   InvalidUtf8(PathBuf),
    #[error("shell command failed: exit {exit_code}")] ShellFailed { exit_code: i32, stderr: String },
    #[error("io error: {0}")]                          Io(#[from] std::io::Error),
    #[error("other: {0}")]                             Other(String),
}

#[derive(thiserror::Error, Debug)]
pub enum SessionError {
    #[error("entry not found: {0}")]               EntryNotFound(EntryId),
    #[error("session not found: {0}")]             SessionNotFound(String),
    #[error("session already exists: {0}")]        SessionAlreadyExists(String),
    #[error("not a leaf: {0}")]                    NotALeaf(EntryId),
    #[error("invalid parent: {0}")]                InvalidParent(EntryId),
    #[error("storage io: {0}")]                    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]                 Serialization(String),
    #[error("concurrent modification")]            ConcurrentModification,
}

#[derive(thiserror::Error, Debug)]
pub enum CompactionError {
    #[error("not enough tokens to compact")]      InsufficientTokens,
    #[error("summary model call failed: {0}")]    SummaryFailed(String),
    #[error(transparent)]                          Session(#[from] SessionError),
    #[error(transparent)]                          Agent(#[from] AgentError),
}

#[derive(thiserror::Error, Debug)]
pub enum TemplateError {
    #[error("template not found: {0}")]                    NotFound(String),
    #[error("missing required argument at position {0}")]  MissingArg(usize),
    #[error("invalid argument syntax: {0}")]               InvalidSyntax(String),
}

#[derive(thiserror::Error, Debug)]
pub enum HarnessError {
    #[error("harness is not idle (current phase: {0:?})")] NotIdle(HarnessPhase),
    #[error("skill not found: {0}")]                        SkillNotFound(String),
    #[error("template not found: {0}")]                     TemplateNotFound(String),
    #[error(transparent)]                                   Agent(#[from] AgentError),
    #[error(transparent)]                                   Session(#[from] SessionError),
    #[error(transparent)]                                   Compaction(#[from] CompactionError),
    #[error(transparent)]                                   Env(#[from] EnvError),
    #[error(transparent)]                                   Template(#[from] TemplateError),
}

#[derive(Debug, Clone, Copy)]
pub enum DiagnosticLevel { Warn, Error }

/// Harness 运行阶段——提升到 types 是因为 HarnessError::NotIdle 需要携带它。
/// AgentPhase 留在 llm-harness 中（AgentError::NotIdle 无 payload）。
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum HarnessPhase { Idle, Turning, Compacting, Branching }
```

### 3.2 ContentBlock 与消息类型

`ContentBlock` 含 Anthropic 风格的 thinking 块——extended thinking 已是 first-class 特性，且 compaction 时必须保留思考痕迹。

```rust
pub enum ContentBlock {
    Text     { text: String },
    Thinking { thinking: String, signature: Option<String> },  // Anthropic extended thinking
    Image    { source: ImageSource },
    ToolUse  { id: String, name: String, input: serde_json::Value },
}

pub enum ImageSource {
    Base64 { media_type: String, data: String },
    // 未来扩展：Url { url: String }, Id { id: String }
}
```

**消息类型——富字段：** AssistantMessage 携带 usage/provider/model/error，这些不是元数据装饰，而是 compaction、回放、错误处理的功能性依赖。

```rust
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    /// 框架内置的特殊摘要消息——compaction 和分支摘要的载体。
    /// 由 convert_to_llm 转换为带特殊前缀的 UserMessage 发给 LLM。
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
    /// 应用层自定义消息——必须由调用方提供 convert_to_llm 转换器才能进入 LLM 上下文
    Custom(CustomMessage),
}

pub struct UserMessage {
    pub content:   Vec<ContentBlock>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct AssistantMessage {
    pub content:       Vec<ContentBlock>,
    pub stop_reason:   Option<StopReason>,
    pub timestamp:     chrono::DateTime<chrono::Utc>,
    pub provider:      Option<String>,        // 来自 llm-api-adapter
    pub api:           Option<String>,        // chat / responses / messages
    pub model:         Option<String>,
    pub usage:         Option<TokenUsage>,    // compaction 估算依赖
    pub error_message: Option<String>,        // 最近一次错误的快照
}

pub struct ToolResultMessage {
    pub tool_use_id: String,
    pub content:     Vec<ContentBlock>,
    pub is_error:    bool,
    pub timestamp:   chrono::DateTime<chrono::Utc>,
}

pub struct BranchSummaryMessage    { pub summary: String, pub timestamp: chrono::DateTime<chrono::Utc> }
pub struct CompactionSummaryMessage { pub summary: String, pub timestamp: chrono::DateTime<chrono::Utc> }

pub struct CustomMessage {
    pub r#type:   String,
    pub data:     serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub struct TokenUsage {
    pub input_tokens:          u32,
    pub output_tokens:         u32,
    pub cache_read_tokens:     u32,
    pub cache_creation_tokens: u32,
}
```

### 3.3 事件模型（消息级 + token 级双层）

事件流既要支持字符流 UI（TextDelta），又要支持消息列表 UI（MessageStart/End 携带完整消息）。两者并存，调用方按需消费。

```rust
pub enum AgentEvent {
    // === Agent 生命周期 ===
    /// agent 开始一次完整运行（一次 prompt 调用）
    AgentStart  { initial_messages: Vec<AgentMessage> },
    /// agent 完成本次运行；包含本次新增的所有消息（Agent ↔ Harness 的关键接口）
    AgentEnd    { new_messages: Vec<AgentMessage> },

    // === Turn 生命周期 ===
    TurnStart   { index: u32 },
    /// 一次 turn 结束；含本轮 assistant message 与 tool 执行结果
    TurnEnd     { index: u32, message: AssistantMessage, tool_results: Vec<(String, Result<ToolResult, ToolError>)> },

    // === 消息级（assistant message 边界） ===
    MessageStart  { message_id: String },
    /// 流式期间，partial assistant message 的当前快照（每次更新覆盖之前的）
    MessageUpdate { message_id: String, partial: AssistantMessage },
    /// 消息完整生成完毕，含 stop_reason 和 usage
    MessageEnd    { message_id: String, message: AssistantMessage },

    // === Token 级（字符流） ===
    TextDelta        { message_id: String, text: String },
    ThinkingDelta    { message_id: String, thinking: String },
    ToolCallStart    { message_id: String, tool_use_id: String, name: String },
    ToolCallArgsDelta { tool_use_id: String, partial_input: String },
    ToolCallEnd      { tool_use_id: String, args: serde_json::Value },

    // === 工具执行（与 ToolCall 不同：ToolCall 是 LLM 发起请求，ToolExecution 是 Rust 执行） ===
    ToolExecutionStart  { tool_use_id: String, tool_name: String, args: serde_json::Value },
    ToolExecutionUpdate { tool_use_id: String, partial: ToolResult },  // 长任务流式
    ToolExecutionEnd    { tool_use_id: String, result: Result<ToolResult, ToolError> },

    Error(AgentError),
}
```

**事件传递语义：** Agent 层使用 `tokio::sync::broadcast` 分发。容量在 `AgentOptions::event_channel_capacity` 中可配（默认 256）。慢消费者会丢失事件——**调用方不应通过事件流重建状态机**；状态机应基于 Session log 或 `MessageEnd`/`AgentEnd` 的完整 payload。

### 3.4 Tool trait

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    /// 人类可读标签，用于 UI 显示（不同于 name 的稳定标识符）
    fn label(&self) -> &str { self.name() }
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> &serde_json::Value;
    fn execution_mode(&self) -> ToolExecutionMode { ToolExecutionMode::Parallel }

    /// 在 schema 校验前对 LLM 原始参数做兼容转换。
    /// 处理参数格式演化（如 LLM 返回的字段名变化）。
    fn prepare_arguments(&self, raw: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        Ok(raw)
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        ctx:  &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>>;
}

pub enum ToolExecutionMode { Parallel, Sequential }

pub struct ToolContext {
    pub env:          Arc<dyn ExecutionEnv>,
    pub abort:        CancellationToken,
    /// 当前 tool call 在 LLM 返回中的 id（用于事件关联）
    pub tool_use_id:  String,
    /// 长时间运行的 tool 可通过此 channel 推送部分结果。
    /// 接收端转发为 AgentEvent::ToolExecutionUpdate。
    pub update_tx:    tokio::sync::mpsc::Sender<ToolResult>,
}

pub struct ToolResult {
    pub content: Vec<ContentBlock>,
    pub details: serde_json::Value,
    /// 当一个 batch 中所有 tool 都返回 terminate=true 时，agent 提前停止 loop。
    /// 允许 tool 自主宣告"任务完成"。
    pub terminate: bool,
}
```

**生命周期：** `execute` 返回的 future 借用 `&self` 和 `&ToolContext`。并行执行通过 `futures::future::join_all` 在 loop 任务内驱动（同任务并发）。

### 3.5 ExecutionEnv trait

对齐 pi-agent-core 的 FileSystem + Shell 能力。路径操作（absolutePath/joinPath/canonicalPath）由 Rust `std::path::Path` 原生处理，**不进 trait**。

```rust
pub trait ExecutionEnv: Send + Sync {
    fn working_dir(&self) -> &Path;

    // === 文件 ===
    fn read_text_file<'a>(&'a self, path: &'a Path, abort: CancellationToken)
        -> BoxFuture<'a, Result<String, EnvError>>;
    fn read_text_lines<'a>(&'a self, path: &'a Path, max_lines: Option<usize>, abort: CancellationToken)
        -> BoxFuture<'a, Result<Vec<String>, EnvError>>;
    fn read_binary_file<'a>(&'a self, path: &'a Path, abort: CancellationToken)
        -> BoxFuture<'a, Result<Vec<u8>, EnvError>>;
    fn write_file<'a>(&'a self, path: &'a Path, content: &'a [u8], abort: CancellationToken)
        -> BoxFuture<'a, Result<(), EnvError>>;
    fn append_file<'a>(&'a self, path: &'a Path, content: &'a [u8], abort: CancellationToken)
        -> BoxFuture<'a, Result<(), EnvError>>;
    fn file_info<'a>(&'a self, path: &'a Path, abort: CancellationToken)
        -> BoxFuture<'a, Result<FileInfo, EnvError>>;
    fn list_dir<'a>(&'a self, path: &'a Path, abort: CancellationToken)
        -> BoxFuture<'a, Result<Vec<FileInfo>, EnvError>>;
    fn exists<'a>(&'a self, path: &'a Path, abort: CancellationToken)
        -> BoxFuture<'a, Result<bool, EnvError>>;
    fn create_dir<'a>(&'a self, path: &'a Path, recursive: bool, abort: CancellationToken)
        -> BoxFuture<'a, Result<(), EnvError>>;
    fn remove<'a>(&'a self, path: &'a Path, recursive: bool, force: bool, abort: CancellationToken)
        -> BoxFuture<'a, Result<(), EnvError>>;
    fn create_temp_dir<'a>(&'a self, prefix: &'a str)
        -> BoxFuture<'a, Result<PathBuf, EnvError>>;

    // === Shell ===
    fn execute_shell<'a>(&'a self, cmd: &'a str, opts: ShellOptions<'a>)
        -> BoxFuture<'a, Result<ShellOutput, EnvError>>;

    /// 释放 env 持有的临时资源（temp dir 等）
    fn cleanup<'a>(&'a self) -> BoxFuture<'a, Result<(), EnvError>>;
}

pub struct ShellOptions<'a> {
    pub cwd:        Option<&'a Path>,
    pub env:        Vec<(&'a str, &'a str)>,
    pub timeout:    Option<Duration>,
    pub abort:      CancellationToken,
    /// 流式 stdout/stderr 回调（None 时仅在最终 Output 中返回完整结果）
    pub on_stdout:  Option<Box<dyn FnMut(&str) + Send + 'a>>,
    pub on_stderr:  Option<Box<dyn FnMut(&str) + Send + 'a>>,
}

pub struct ShellOutput { pub stdout: String, pub stderr: String, pub exit_code: i32 }
pub struct FileInfo   { pub path: PathBuf, pub is_dir: bool, pub size: u64, pub modified: chrono::DateTime<chrono::Utc> }
```

**权限模型：** trait 不提供细粒度权限；由实现方控制（OsEnv 可配工作目录边界、shell 白名单）。需要更细控制的调用方应包装受限 env 注入 ToolContext。

### 3.6 Hook traits

> **架构原则：** `ConvertToLlmHook` 涉及外部 crate `llm-api-adapter::Message`，违反 types 的零 IO 约束，**定义在 `llm-harness-loop` 中**（详见 §4.1）。其他 hook 因不依赖外部类型，保留在 types 中。

```rust
/// 每次 LLM 调用前对上下文做转换（compaction 通过此 hook 接入）。
pub trait TransformContextHook: Send + Sync {
    fn transform<'a>(&'a self, ctx: AgentContext)
        -> BoxFuture<'a, Result<AgentContext, AgentError>>;
}

/// 每个 turn 结束后调用，可返回新的 context / model / thinking_level / tools。
/// AgentHarness 用它每轮从 session log 重建上下文。
pub struct PrepareNextTurnCtx<'a> {
    pub turn_index:        u32,
    pub last_message:      &'a AssistantMessage,
    pub last_tool_results: &'a [(String, Result<ToolResult, ToolError>)],
}
pub struct NextTurnDirective {
    pub context:        Option<AgentContext>,
    pub model:          Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub tools:          Option<Vec<Arc<dyn Tool>>>,            // 替换全部工具
    pub active_tools:   Option<HashSet<String>>,               // 仅控制激活子集
}
pub trait PrepareNextTurnHook: Send + Sync {
    fn prepare<'a>(&'a self, ctx: PrepareNextTurnCtx<'a>)
        -> BoxFuture<'a, Result<NextTurnDirective, AgentError>>;
}

pub struct BeforeToolCallCtx<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_use_id:       &'a str,
    pub tool_name:         &'a str,
    pub args:              &'a serde_json::Value,
    pub turn_index:        u32,
}
pub enum BeforeToolCallDecision {
    Allow,
    Modify(serde_json::Value),
    Deny(ToolResult),
}
pub trait BeforeToolCallHook: Send + Sync {
    fn on_call<'a>(&'a self, ctx: BeforeToolCallCtx<'a>) -> BoxFuture<'a, BeforeToolCallDecision>;
}

pub struct AfterToolCallCtx<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_use_id:       &'a str,
    pub tool_name:         &'a str,
    pub args:              &'a serde_json::Value,
    pub result:            &'a Result<ToolResult, ToolError>,
    pub turn_index:        u32,
}
pub trait AfterToolCallHook: Send + Sync {
    fn on_complete<'a>(&'a self, ctx: AfterToolCallCtx<'a>) -> BoxFuture<'a, ()>;
}

pub struct ShouldStopCtx<'a> {
    pub last_assistant: &'a AssistantMessage,
    pub stop_reason:    StopReason,
    pub turn_index:     u32,
}
pub trait ShouldStopHook: Send + Sync {
    /// 仅在 LLM 自然停止时调用。返回 true 才停止；返回 false 强制再跑一轮。
    /// 不能用于强制中断进行中的 turn——中断走 abort()。
    fn should_stop<'a>(&'a self, ctx: ShouldStopCtx<'a>) -> BoxFuture<'a, bool>;
}

/// Provider 请求拦截：可改 stream options
pub trait BeforeProviderRequestHook: Send + Sync {
    fn before_request<'a>(&'a self, opts: &'a mut StreamOptions) -> BoxFuture<'a, ()>;
}

pub struct ProviderResponseInfo {
    pub status_code:      Option<u16>,
    pub response_headers: Vec<(String, String)>,
    pub usage:            Option<TokenUsage>,
    pub latency_ms:       u64,
}
pub trait AfterProviderResponseHook: Send + Sync {
    fn after_response<'a>(&'a self, info: &'a ProviderResponseInfo) -> BoxFuture<'a, ()>;
}

/// API key / headers 动态解析（OAuth token 过期等场景）
pub trait AuthHook: Send + Sync {
    fn resolve<'a>(&'a self) -> BoxFuture<'a, Result<AuthInfo, AgentError>>;
}
pub struct AuthInfo { pub api_key: Option<String>, pub headers: Vec<(String, String)> }

/// Harness 专属：turn 边界
pub struct BeforeTurnCtx<'a> { pub turn_index: u32, pub snapshot: &'a TurnSnapshot }
pub struct AfterTurnCtx<'a>  { pub turn_index: u32, pub new_messages: &'a [AgentMessage] }
pub trait BeforeTurnHook: Send + Sync { fn before_turn<'a>(&'a self, ctx: BeforeTurnCtx<'a>) -> BoxFuture<'a, ()>; }
pub trait AfterTurnHook:  Send + Sync { fn after_turn<'a>(&'a self, ctx: AfterTurnCtx<'a>) -> BoxFuture<'a, ()>; }

/// Compaction 决策点
pub struct BeforeCompactCtx<'a> { pub estimated_tokens: usize, pub messages: &'a [AgentMessage] }
pub enum BeforeCompactDecision { Proceed, Skip, Override(CompactionResult) }
pub trait BeforeCompactHook: Send + Sync {
    fn before_compact<'a>(&'a self, ctx: BeforeCompactCtx<'a>) -> BoxFuture<'a, BeforeCompactDecision>;
}
```

### 3.7 其他基础类型

```rust
pub enum ThinkingLevel { Off, Minimal, Low, Medium, High, XHigh }

pub struct AgentContext {
    pub system_prompt: Option<String>,
    pub messages:      Vec<AgentMessage>,
}

/// Turn 快照：types 层的纯值结构。Agent / AgentHarness 在 turn 开始时
/// 用各自的 *State 直接构造（在持锁块内 clone 字段），不需 From impl。
/// 所有字段 pub 允许 workspace 内任意 crate 直接构造。
#[derive(Clone)]
pub struct TurnSnapshot {
    pub model:          String,
    pub thinking_level: ThinkingLevel,
    pub tools:          Vec<Arc<dyn Tool>>,
    pub system_prompt:  Option<String>,
}

/// Provider 流式传输配置——可被 BeforeProviderRequestHook 覆盖
pub struct StreamOptions {
    pub timeout_ms:           Option<u64>,
    pub max_retries:           Option<u32>,
    pub max_retry_delay_ms:    Option<u64>,
    pub headers:               Vec<(String, String)>,
    pub metadata:              serde_json::Value,
    pub cache_config:          Option<serde_json::Value>,  // 厂商特定
}
```
