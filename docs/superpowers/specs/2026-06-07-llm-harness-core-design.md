# llm-harness-core 系统设计

**日期：** 2026-06-07
**状态：** 已批准（v6，纳入模块边界审核修订）

## 1. 背景与目标

本项目是 [`@earendil-works/pi-agent-core`](../../../../pi-main/packages/agent) TypeScript 包的 Rust 全量重写。目标不是逐行翻译，而是学习其核心设计哲学，用 Rust 惯用法重新表达。

核心目标：
- 提供完整的 agent 运行时：低层循环、有状态 Agent、编排层 AgentHarness
- 全量对标 pi-agent-core 功能：会话持久化（含分支前向兼容）、上下文压缩、skills/templates、执行环境抽象
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

### 5.3 Session（多分支树）

**设计哲学：** 会话是类型化追加日志，每条 entry 含 `parent_id` 形成**真正的树**。多个 leaf 表示并存的分支；用户可在任意历史 entry 上 fork 出新分支；`navigate_to(target)` 切换写入位置。

**核心概念：**
- **Entry tree**：所有 entry 按 `parent_id` 链接成树（root 的 parent = None）
- **Leaf**：任何一个没有子 entry 的节点；每条分支对应一个 leaf
- **Active cursor** (`active_cursor`)：下一次 append 时新 entry 的 `parent_id` 指向。命名**不**用 "active_leaf"——fork 操作会把 cursor 临时指向树的内部节点（非 leaf），下一条 append 才创造出新 leaf。
- **Branch**：从 root 到任一 leaf 的路径
- **Fork**：把 cursor 指向某历史 entry 后追加，新 entry 自然成为新分支的起点
- **Cross-session fork**：把整条路径复制到新 session 作为独立时间线

```rust
pub struct SessionEntry {
    pub id:         EntryId,
    pub parent_id:  Option<EntryId>,    // None = root
    pub timestamp:  chrono::DateTime<chrono::Utc>,
    pub payload:    SessionEntryPayload,
}

pub enum SessionEntryPayload {
    Message(AgentMessage),
    ModelChange         { to: String, provider: Option<String>, model_id: Option<String> },
    ThinkingLevelChange { to: ThinkingLevel },
    ActiveToolsChange   { active: Vec<String> },
    Compaction(CompactionEntry),
    Label               { name: String },
    SessionInfo         { name: String },             // 会话命名（UI 显示）
    Custom              { r#type: String, data: serde_json::Value },

    /// 分支点标记：明示此 entry 之后产生了新分支（导航 UI 用）
    /// 不强制——任何 entry 都可以成为分支起点；本 entry 仅做语义标注
    BranchPoint  { from: EntryId, label: Option<String> },

    /// 分支切换记录：从 from cursor 切换到 to cursor（写入新分支的第一条 entry）
    BranchSwitch { from: EntryId, to: EntryId, summary: Option<String> },

    /// 分支摘要：AI 生成的某个分支的概要，导航时辅助理解上下文
    BranchSummary(BranchSummaryEntry),
}

pub struct CompactionEntry {
    pub summary_message:   AgentMessage,
    pub first_kept_entry:  EntryId,
    pub tokens_before:     usize,
    pub from_hook:         bool,
    pub details:           Option<serde_json::Value>,
}

pub struct BranchSummaryEntry {
    pub leaf_id:        EntryId,
    pub from_entry:     EntryId,
    pub summary:        String,
    pub token_count:    usize,
}
```

**存储层：**

```rust
pub struct SessionMetadata {
    pub id:                  String,
    pub name:                Option<String>,
    pub created_at:          chrono::DateTime<chrono::Utc>,
    pub updated_at:          chrono::DateTime<chrono::Utc>,
    pub model:               Option<String>,
    pub active_cursor:       Option<EntryId>,     // 当前写入位置（可能是 leaf，也可能是 fork 后的内部节点）
    pub parent_session_path: Option<String>,      // 跨 session fork 时引用的 source（copy_entries=false 场景）
}

pub struct CreateSessionOptions {
    pub name:           Option<String>,
    pub initial_model:  Option<String>,
    pub initial_thinking_level: Option<ThinkingLevel>,
    pub initial_tools:  Vec<String>,
}

pub struct ListSessionOptions {
    pub limit:          Option<usize>,
    pub offset:         Option<usize>,
    pub order:          ListOrder,
    pub name_contains:  Option<String>,
}
pub enum ListOrder { CreatedAsc, CreatedDesc, UpdatedAsc, UpdatedDesc }

/// 底层存储 trait：负责字节追加 + 树查询 + 活跃 cursor 追踪。
/// 实现方负责内部串行化——append/set_active_cursor 等写操作必须原子。
pub trait SessionStorage: Send + Sync {
    fn metadata<'a>(&'a self) -> BoxFuture<'a, Result<SessionMetadata, SessionError>>;

    fn create_entry_id(&self) -> EntryId;  // UUIDv7

    fn append_entry<'a>(&'a self, entry: SessionEntry)
        -> BoxFuture<'a, Result<(), SessionError>>;

    fn get_entry<'a>(&'a self, id: EntryId)
        -> BoxFuture<'a, Result<Option<SessionEntry>, SessionError>>;

    fn children<'a>(&'a self, parent: EntryId)
        -> BoxFuture<'a, Result<Vec<SessionEntry>, SessionError>>;

    fn all_leaves<'a>(&'a self)
        -> BoxFuture<'a, Result<Vec<EntryId>, SessionError>>;

    /// 当前写入位置
    fn active_cursor<'a>(&'a self)
        -> BoxFuture<'a, Result<Option<EntryId>, SessionError>>;

    fn set_active_cursor<'a>(&'a self, id: EntryId)
        -> BoxFuture<'a, Result<(), SessionError>>;

    fn path_to_root<'a>(&'a self, target: EntryId)
        -> BoxFuture<'a, Result<Vec<SessionEntry>, SessionError>>;

    fn common_ancestor<'a>(&'a self, a: EntryId, b: EntryId)
        -> BoxFuture<'a, Result<Option<EntryId>, SessionError>>;

    fn label_at<'a>(&'a self, id: EntryId)
        -> BoxFuture<'a, Result<Option<String>, SessionError>>;

    fn find_entries_by_type<'a>(&'a self, kind: &'a str)
        -> BoxFuture<'a, Result<Vec<EntryId>, SessionError>>;
}

/// 仓库抽象——管理多个 session 的生命周期与跨 session fork。
pub trait SessionRepo: Send + Sync {
    fn create<'a>(&'a self, opts: CreateSessionOptions)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;

    fn open<'a>(&'a self, id: &'a str)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;

    fn list<'a>(&'a self, opts: ListSessionOptions)
        -> BoxFuture<'a, Result<Vec<SessionMetadata>, SessionError>>;

    fn delete<'a>(&'a self, id: &'a str)
        -> BoxFuture<'a, Result<(), SessionError>>;

    /// 跨 session fork：把 source 中从 root 到 from_entry 的整条路径
    /// 作为新 session 的初始内容。v1 仅支持 copy_entries=true（实体复制）。
    fn fork<'a>(&'a self, source_id: &'a str, from_entry: EntryId, opts: ForkOptions)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;
}

pub struct ForkOptions {
    pub name:         Option<String>,
    /// v1 强制为 true（完整复制 entry，重新分配 id）。
    /// false 的引用模式涉及跨 session entry 引用，v1.x 实现。
    pub copy_entries: bool,
}

/// 高层 Session 接口：解释 Compaction，构建"当前有效上下文"。
/// 内部通过持有 storage 的 Arc 与可选 Mutex 串行化写操作。
pub struct Session {
    storage: Arc<dyn SessionStorage>,
}

impl Session {
    /// 从 active_cursor 回溯到 root 的原始路径
    pub async fn read_active_path(&self) -> Result<Vec<SessionEntry>, SessionError>;

    /// 任意 leaf 的原始路径
    pub async fn read_path_of(&self, leaf: EntryId)
        -> Result<Vec<SessionEntry>, SessionError>;

    /// 当前活跃路径的"有效上下文"——解释 Compaction，附带最后已知配置
    pub async fn build_context(&self) -> Result<BuiltContext, SessionError>;

    /// 在 active_cursor 下追加 message（最常用）。
    /// 内部：调 storage.create_entry_id 生成 id，读 storage.active_cursor() 作为 parent_id，
    /// 使用 Utc::now() 作为 timestamp，写入后将 active_cursor 更新为新 entry id。
    pub async fn append_message(&self, msg: AgentMessage) -> Result<EntryId, SessionError>;

    /// 追加任意 payload，行为同上
    pub async fn append(&self, payload: SessionEntryPayload) -> Result<EntryId, SessionError>;

    // === 分支操作 ===

    /// 切换 active_cursor 到指定 entry（可以是 leaf 或任意历史节点）。
    /// 写入一条 BranchSwitch entry 作为切换记录。
    pub async fn navigate_to(&self, target: EntryId) -> Result<(), SessionError>;

    /// 在历史 entry 上 fork 新分支：
    /// 1. 将 active_cursor 切到 from_entry
    /// 2. 写入一条 BranchPoint entry 标记新分支起点
    /// 3. 之后的 append 会以 BranchPoint 为 parent_id 形成新分支
    pub async fn fork_branch(&self, from_entry: EntryId, label: Option<String>)
        -> Result<EntryId, SessionError>;

    /// 列出所有分支（每个 leaf 对应一条）
    pub async fn list_branches(&self) -> Result<Vec<BranchInfo>, SessionError>;

    /// 删除一个分支：从指定 leaf 开始向上删除，直到遇到有其他子节点的 entry
    pub async fn delete_branch(&self, leaf: EntryId) -> Result<(), SessionError>;
}

pub struct BuiltContext {
    pub messages:           Vec<AgentMessage>,
    pub last_model:         Option<String>,
    pub last_thinking_level: Option<ThinkingLevel>,
    pub last_active_tools:  Option<Vec<String>>,
}

pub struct BranchInfo {
    pub leaf_id:       EntryId,
    pub label:         Option<String>,
    pub message_count: usize,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub summary:       Option<String>,
}

pub struct JsonlSessionRepo  { root_dir: PathBuf }
pub struct InMemorySessionRepo { /* ... */ }
```

**并发安全：** `Session` 的方法签名是 `&self`——允许多 caller 共享 `Arc<Session>`。串行化在 `SessionStorage` 实现内：
- `JsonlSessionStorage` 内部持有 `tokio::sync::Mutex<Internal>`，所有 append/set_active_cursor/path query 串行化
- 高层组合操作（如 `fork_branch` = set_active_cursor + append BranchPoint）在 Session 内通过持有同一锁保证两步原子
- 调用方仍需注意逻辑层并发——同时 `navigate_to(A)` 与 `navigate_to(B)` 不会损坏存储，但最终落到哪个是非确定性的

**JSONL 存储策略 + 缓存：**
- 单 session 单文件，所有 entry 按时间 append；树结构由 `parent_id` 重建
- `active_cursor` + metadata 持久化到独立的 `.meta.json`
- `JsonlSessionStorage` 内部维护 in-memory 树缓存（`HashMap<EntryId, SessionEntry>` + `HashMap<EntryId, Vec<EntryId>>` 子节点表）：
  - 首次访问时 mmap/读全量文件构建
  - 每次 `append_entry` 增量更新缓存（O(1) 插入）
  - `path_to_root` / `children` / `all_leaves` 直接走内存，O(path length) 或 O(1)
- 大文件（>10k entries）通过 fs::Metadata::len 触发懒加载策略

**分支摘要生成：** `generate_branch_summary(client, session, leaf)` 在 Harness 层提供，调用 `summary_model` 生成 BranchSummary entry。

**Compaction 与分支的交互：** 每个分支独立追踪 compaction 历史——`first_kept_entry` 沿 parent_id 链向上追溯，找到本分支最近一次 Compaction entry 作为基准。fork 出的新分支继承父分支的 compaction 历史。

### 5.4 Compaction（基于 session entries）

**关键修正：** Compaction 操作的是 **session path entries**，而非纯 message 数组。这样才能：
1. 在路径中定位上次 compaction 的 `first_kept_entry`
2. Cut point 落在 entry 边界，不在 toolResult 中间截断
3. 输出新的 `first_kept_entry` 作为下一次 compaction 边界

```rust
pub struct CompactionSettings {
    pub enabled:            bool,
    /// 触发条件：token 数 > `model_info.context_window - reserve_tokens`
    pub reserve_tokens:     usize,
    /// 保留尾部 N tokens 不压缩
    pub keep_recent_tokens: usize,
    pub summary_model:      String,
    /// 摘要模型的完整元数据——compaction 必需用于 token 估算
    pub summary_model_info: ModelInfo,
}

/// 中间类型——分离决策（是否压缩、切点）与执行（调 LLM 生成摘要）
pub struct CompactionPreparation {
    pub path_entries:      Vec<SessionEntry>,
    pub first_kept_entry:  EntryId,
    pub cut_point:         EntryId,         // 此 entry 之前的全部压缩
    pub previous_summary:  Option<String>,  // 上次 compaction 的摘要，用于迭代更新
    pub estimated_tokens:  usize,
    pub split_turn_prefix: Option<Vec<SessionEntry>>, // 跨 turn 边界时的独立摘要
    pub file_operations:   Vec<FileOperation>,        // 跨 compaction 累积的文件读写
}

pub fn prepare_compaction(
    path:            &[SessionEntry],
    last_compaction: Option<&CompactionEntry>,
    settings:        &CompactionSettings,
    model_info:      &ModelInfo,   // 主对话的 model info，用于估算阈值
) -> Option<CompactionPreparation>;  // None = 无需压缩

/// `auth` 为可选——若 None，使用 `client` 自身已配置的认证；
/// 若 Some，每次调用前调用 hook 刷新 api_key/headers（OAuth 场景）。
pub async fn compact(
    client:      &dyn LlmClient,
    preparation: CompactionPreparation,
    settings:    &CompactionSettings,
    auth:        Option<&dyn AuthHook>,
) -> Result<CompactionResult, CompactionError>;

pub struct CompactionResult {
    pub summary_message:   AgentMessage,    // CompactionSummaryMessage
    pub first_kept_entry:  EntryId,         // 写入 CompactionEntry
    pub tokens_before:     usize,
    pub tokens_after:      usize,
    pub file_operations:   Vec<FileOperation>,
}

pub struct FileOperation {
    pub path:       PathBuf,
    pub kind:       FileOpKind,  // Read | Modify
    pub at_entry:   EntryId,
}
```

**LLM 调用归属：** 独立于 Agent 主循环，由 Harness 用 `summary_model` 直接调 `LlmClient`。Agent 不感知。

**阶段交互：** v1 `harness.compact()` 仅在 Harness Idle 时可调用，期间转为 `Compacting`。v1.x 可考虑后台压缩。

### 5.5 Skills 与 PromptTemplates

#### Skills

```rust
pub struct Skill {
    pub name:                     String,   // 校验：小写字母+数字+连字符，≤64
    pub label:                    Option<String>,
    pub description:              String,   // 校验：非空，≤1024
    pub content:                  String,
    pub source:                   PathBuf,
    pub disable_model_invocation: bool,     // true: 不进 system prompt，仅供显式调用
}

pub struct SkillDiagnostic { pub source: PathBuf, pub level: DiagnosticLevel, pub message: String }
// DiagnosticLevel 定义见 §3.1

pub struct SourcedSkill {
    pub skill:  Skill,
    pub source_tag: String,  // 调用方提供的来源标记（如 "user-config", "project-local"）
}

/// 递归扫描目录；每目录只取第一个 SKILL.md（不递归子目录的 SKILL.md）。
/// 校验 name 匹配父目录名。遵守 .gitignore / .ignore / .fdignore。
/// 解析符号链接。
pub async fn load_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<Skill>, Vec<SkillDiagnostic>);

pub async fn load_sourced_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[(String, PathBuf)],  // (source_tag, dir)
) -> (Vec<SourcedSkill>, Vec<SkillDiagnostic>);

/// 注入 system prompt（仅 disable_model_invocation=false 的 skill）
pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String;

/// 显式调用：将 skill 内容包装为 `<skill name="...">...</skill>` 块，作为 user 消息注入
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String;
```

#### PromptTemplates

**位置参数 + shell-style 引号解析**，对齐 pi-agent-core：占位符 `$1`、`$2`、`$@`、`$ARGUMENTS`、`${@:N}`、`${@:N:L}`。

```rust
pub struct PromptTemplate {
    pub name:    String,
    pub content: String,
    pub source:  PathBuf,
}

pub async fn load_prompt_templates(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<PromptTemplate>, Vec<SkillDiagnostic>);

/// args 为位置参数列表；invoke 内部 shell-style 解析输入
pub fn invoke_template(
    template: &PromptTemplate,
    args:     &[String],
) -> Result<String, TemplateError>;
```

**测试策略：** Skills 和 Templates 的加载逻辑依赖 `dyn ExecutionEnv`。v1 **不**提供 `InMemoryEnv` mock——测试通过 `tempfile::TempDir` + `OsEnv` 真实运行：在临时目录布置 `SKILL.md` / 模板文件，调用 `load_skills()`，验证返回值。这样既覆盖了真实 fs 行为，又避免维护双重 env 实现。后续若需要 hermetic 测试再引入 `InMemoryEnv`。

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

## 6. 关键设计决策汇总

| 决策 | 选择 | 理由 |
|---|---|---|
| 事件模型 | 消息级 + token 级双层（含 AgentStart/End、MessageStart/End 携 payload） | 同时支持消息列表 UI 与字符流 UI；Agent↔Harness 通过 AgentEnd payload 传递结果 |
| ConvertToLlmHook 位置 | 定义在 `llm-harness-loop`（不在 types） | types crate 维持零 IO 约束 |
| Hook 真相源 | HarnessHooks 唯一源；Harness 每次构造临时 LoopConfig | 消除字段重复，明确翻译路径 |
| HookedTool | tool wrapper，承载 before/after_tool_call | Loop 层不接受 tool call hook |
| next_turn 注入 | HarnessState.queued_next_turn 缓冲，下次 prompt 时合并 | 无需新 channel |
| Steer/follow_up 类型 | `AgentMessage` channel + `&str` 便捷方法 | 支持多模态（图片等） |
| active_cursor 命名 | 弃用 active_leaf——fork 后会指向非 leaf | 命名准确反映"下次 append 的 parent" |
| Session::append 内部 | 自动填 id（storage.create_entry_id）/parent_id（active_cursor）/timestamp | 调用方零负担 |
| Session 并发 | Storage 内部 Mutex 串行化；高层组合操作持同一锁 | JSONL append + meta 更新原子 |
| JSONL 缓存 | Storage 内部维护内存树缓存，append 增量更新 | 避免每次树查询全量扫描 |
| CompactionSettings | 删除 token_threshold，触发条件统一为 context_window - reserve_tokens | 消除语义重叠；强制 summary_model_info |
| BuiltContext | build_context 返回 messages + 最后已知 model/thinking/tools | session 重建完整运行时配置 |
| 事件处理管道 | Harness 内部明确伪代码定义事件→pending writes→save point 流转 | 核心正确性可审计 |
| 消息富类型 | AssistantMessage 携 usage/timestamp/provider/model/error | compaction 估算、回放、错误处理需要 |
| ThinkingContent | 一等公民 ContentBlock variant | Anthropic extended thinking 必需，compaction 时保留思考 |
| Custom message | 框架内置 BranchSummary/CompactionSummary 为具名 variant；其他走 CustomMessage + 必需 ConvertToLlmHook | 类型安全 + 灵活扩展 |
| 工具定义 | `dyn Tool` trait，含 label / prepare_arguments / onUpdate channel / terminate | UI 友好 + LLM 参数兼容 + 流式工具输出 + 自主停止 |
| 工具调度 | 分治：按 Sequential 切子组，组内并发 | 避免连坐降级 |
| 执行环境 | 完整 trait（~13 方法）+ ShellOptions | 对齐 TS 的 FileSystem/Shell；路径操作走 std::path |
| 锁策略 | `std::sync::Mutex` + 快照模式 | 性能 + 跨 await 安全 |
| 阶段锁 | 运行时枚举 | typestate 不适合 async + 长生命周期 |
| Session 结构 | 真正的多分支树（parent_id + active_leaf + all_leaves） | v1 即支持 fork/navigate/list/delete 分支 |
| Session 仓库 | `SessionRepo` trait（create/open/list/delete/fork） | 多 session 管理；fork 跨 session 复制路径 |
| Session 读取 | `read_active_path` 原始 + `build_context` 解释 Compaction + `read_path_of(leaf)` 任意分支 | 双层职责 + 多分支支持 |
| 分支摘要 | `generate_branch_summary` 用 summary_model 生成；写入 BranchSummary entry | 导航 UI 显示分支概要 |
| Compaction | 基于 session entries；prepare/execute 两段；输出 first_kept_entry | 边界正确性 + 迭代摘要 |
| convert_to_llm | LoopConfig 必需 hook | CustomMessage 不能直送 LLM |
| prepare_next_turn | LoopConfig 可选 hook | Harness 从 session 重建上下文 |
| continue_run | Agent 与 loop 双层支持 | prepareNextTurn 基础 |
| AgentHarness 架构 | 直接驱动 loop，不包装 Agent | 对齐 TS 的"超集替代"定位 |
| Hook 数量 | 约 11 个语义化 hook + AgentHarnessEvent enum 通知 | 覆盖 TS 主要扩展点，避免事件膨胀 |
| 动态认证 | `AuthHook` 在 LoopConfig 与 Harness 均可挂入 | OAuth token 过期等场景 |
| StreamOptions | 显式结构传入 LoopConfig，可被 BeforeProviderRequestHook 覆盖 | 传输层配置可观测可修改 |
| Skill 加载 | 名称/描述校验 + disableModelInvocation + 显式调用 | 安全 + 灵活 |
| PromptTemplate | 位置参数 + shell-style 引号解析 | 对齐现有模板生态 |
| 图片引用 | `ImageSource` enum 预留 URL/Id | 避免 base64 锁死 |
| Crate 拆分 | workspace + 3 crates | 关注点分离 |
| HarnessPhase 位置 | types crate（与 HarnessError 同级） | HarnessError::NotIdle 需携带 HarnessPhase；避免跨 crate 引用编译错 |
| LlmClient 签名 | spec 中以注释形式给出假定接口 | loop 实施可直接开始，不阻塞于 llm-api-adapter 接口落地 |
| ModelInfo 暴露 | loop crate 通过 `pub use` 重导出 LlmClient/ModelInfo/LlmMessage/StreamEvent | harness 不直接依赖 llm-api-adapter |
| Skills/Templates 测试 | tempfile + OsEnv 真实 fs，无 InMemoryEnv | 避免维护双重 env 实现 |

## 7. 不在范围内（v1）

- **Proxy 模式**（browser → backend 流式转发）：可后续作为独立 feature
- **WASM 目标**：`ExecutionEnv` trait 已抽象；WASM 实现留待后期
- **后台 compaction**：v1 同步触发；v1.x 可考虑异步
- **细粒度权限模型**（capability token）：v1 由 ExecutionEnv 实现方控制
- **超大 entry 外部存储**：v1 调用方截断
- **`prepareArguments` 的 typed schema 泛型**：v1 用 `serde_json::Value`，未来可引入 typed 泛型 Tool
- **Agent loop 以外的框架能力**（规划、记忆管理）：调用方责任
- **典型应用层组件**（如内置 bash tool / file edit tool）：作为示例代码或独立 crate 提供
- **分支可视化 UI**：框架提供数据接口（`list_branches`、tree 查询），UI 由调用方实现

## 8. 实施路线图

依赖拓扑：`types → loop → harness`。harness 内部进一步分为可并行的子系统。

```
阶段 1 (types) ─────────────────────────────────────────────────
                │
                ▼
阶段 2 (loop) ──────────────────────────────────────────────────
                │
                ▼
        ┌───────┴──────────┬──────────────────┐
        ▼                  ▼                  ▼
阶段 3 (Agent)    阶段 4 (Session)    阶段 6 (Skills/Templates)
        │                  │                  │
        │                  ▼                  │
        │          阶段 5 (Compaction)        │
        │                  │                  │
        └──────────────────┴──────────────────┘
                           │
                           ▼
                阶段 7 (AgentHarness)
```

| 阶段 | 内容 | 关键测试 | 阻塞依赖 |
|---|---|---|---|
| 1 | types 全部类型 + trait 声明 | 纯数据 derive 与序列化 roundtrip | 无 |
| 2 | LoopConfig / HookedTool / agent_loop / agent_loop_continue / DefaultConvertToLlm | `MockLlmClient` 驱动的事件序列正确性 + tool batch 分治 | 阶段 1 完成 |
| 3 | Agent / AgentState / 队列方法 / continue_run / reset | 状态机转换、abort、并发 prompt、subscribe 完整事件 | 阶段 2 完成 |
| 4 | SessionEntry / SessionStorage / SessionRepo / Session / 内存 + JSONL 实现 / 分支操作 | 树操作、fork、navigate、build_context、并发 append | 阶段 1 完成 |
| 5 | CompactionSettings / prepare_compaction / compact / FileOperation | 纯函数 cut point + MockLlmClient 集成 | 阶段 4 数据结构稳定 |
| 6 | Skills / PromptTemplates / load_skills / invoke_template | tempfile + OsEnv 真实 fs 加载 | 阶段 1 完成 |
| 7 | AgentHarness / HarnessHooks / 事件管道 / 分支 API / next_turn 注入 | 集成测试：prompt → tool → session 写入 → compact → fork → navigate | 阶段 2-6 全部完成 |

**并行机会：** 阶段 3、4、6 可在阶段 2 完成后同时启动（三者互不依赖）。阶段 5 在阶段 4 的 SessionEntry / SessionStorage 数据结构定稿后即可启动。

**测试基础设施（实施任务，不进 spec 主体）：**
- loop crate 提供 feature-gated `test-utils` 模块导出 `MockLlmClient`
- harness 集成测试用 `InMemorySessionRepo` + `MockLlmClient` + `tempfile::TempDir`
