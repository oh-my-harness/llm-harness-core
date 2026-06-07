# llm-harness-core 系统设计

**日期：** 2026-06-07
**状态：** 已批准（v3，已纳入对齐审核 gap-analysis）

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

**依赖：** `serde`, `serde_json`, `futures`（`BoxFuture`），`tokio-util`（`CancellationToken`），`thiserror`，`uuid`，`chrono`

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

```rust
/// AgentMessage → llm-api-adapter Message 的转换。必需 hook（CustomMessage 不能直接送 LLM）。
/// 默认实现处理 User/Assistant/ToolResult/BranchSummary/CompactionSummary；
/// CustomMessage 必须由调用方覆盖处理。
pub trait ConvertToLlmHook: Send + Sync {
    fn convert<'a>(
        &'a self,
        messages: &'a [AgentMessage],
    ) -> BoxFuture<'a, Result<Vec<llm_api_adapter::Message>, AgentError>>;
}

/// 每次 LLM 调用前对上下文做转换（compaction 通过此 hook 接入）。
pub trait TransformContextHook: Send + Sync {
    fn transform<'a>(&'a self, ctx: AgentContext)
        -> BoxFuture<'a, Result<AgentContext, AgentError>>;
}

/// 每个 turn 结束后调用，可返回新的 context / model / thinking_level。
/// AgentHarness 用它每轮从 session log 重建上下文。
pub struct PrepareNextTurnCtx<'a> {
    pub turn_index:        u32,
    pub last_message:      &'a AssistantMessage,
    pub last_tool_results: &'a [(String, Result<ToolResult, ToolError>)],
}
pub struct NextTurnDirective {
    pub context:        Option<AgentContext>,         // None = 沿用当前
    pub model:          Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
}
pub trait PrepareNextTurnHook: Send + Sync {
    fn prepare<'a>(&'a self, ctx: PrepareNextTurnCtx<'a>)
        -> BoxFuture<'a, Result<NextTurnDirective, AgentError>>;
}

pub struct BeforeToolCallCtx<'a> { /* ... assistant_message, tool_use_id, tool_name, args ... */ }
pub enum BeforeToolCallDecision { Allow, Modify(serde_json::Value), Deny(ToolResult) }
pub trait BeforeToolCallHook: Send + Sync {
    fn on_call<'a>(&'a self, ctx: BeforeToolCallCtx<'a>) -> BoxFuture<'a, BeforeToolCallDecision>;
}

pub struct AfterToolCallCtx<'a> { /* ... assistant_message, tool_use_id, tool_name, args, result ... */ }
pub trait AfterToolCallHook: Send + Sync {
    fn on_complete<'a>(&'a self, ctx: AfterToolCallCtx<'a>) -> BoxFuture<'a, ()>;
}

pub struct ShouldStopCtx<'a> { /* last_assistant, stop_reason, turn_index ... */ }
pub trait ShouldStopHook: Send + Sync {
    /// 仅在 LLM 自然停止时调用。返回 true 才停止；返回 false 强制再跑一轮。
    /// 不能用于强制中断进行中的 turn——中断走 abort()。
    fn should_stop<'a>(&'a self, ctx: ShouldStopCtx<'a>) -> BoxFuture<'a, bool>;
}

/// Provider 请求拦截：可改 stream options（timeout、retry、headers、metadata、cache）
pub trait BeforeProviderRequestHook: Send + Sync {
    fn before_request<'a>(&'a self, opts: &'a mut StreamOptions) -> BoxFuture<'a, ()>;
}
/// Provider 响应观察：检视 headers/status，用于配额追踪
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

### 4.1 API

```rust
pub struct LoopConfig {
    pub tools:                  Vec<Arc<dyn Tool>>,
    pub default_execution_mode: ToolExecutionMode,
    pub stream_options:         StreamOptions,

    /// AgentMessage → llm-api-adapter Message 的转换（必需，loop 不会假设默认）
    pub convert_to_llm:         Arc<dyn ConvertToLlmHook>,

    /// 每次 LLM 调用前转换上下文（compaction 通过此 hook 接入）
    pub transform_context:      Option<Arc<dyn TransformContextHook>>,

    /// 每个 turn 后返回新 context/model/thinking_level（Harness 用此从 session 重建上下文）
    pub prepare_next_turn:      Option<Arc<dyn PrepareNextTurnHook>>,

    /// LLM 自然停止时决定是否继续
    pub should_stop:            Option<Arc<dyn ShouldStopHook>>,

    /// Provider 请求/响应拦截
    pub before_provider_request: Option<Arc<dyn BeforeProviderRequestHook>>,
    pub after_provider_response: Option<Arc<dyn AfterProviderResponseHook>>,

    /// 动态认证
    pub auth:                   Option<Arc<dyn AuthHook>>,

    /// 响应式注入 channels
    pub steer_rx:               Option<tokio::sync::mpsc::Receiver<String>>,
    pub follow_up_rx:           Option<tokio::sync::mpsc::Receiver<String>>,
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

**注：** Tool call 拦截 hook（`BeforeToolCallHook` / `AfterToolCallHook`）**仅在 Harness 层挂入**；Harness 通过 `HookedTool` 包装每个 `Arc<dyn Tool>` 实现，Loop 保持纯净。

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
    steer_tx:     tokio::sync::mpsc::Sender<String>,
    follow_up_tx: tokio::sync::mpsc::Sender<String>,
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
pub fn steer(&self, text: impl Into<String>);
pub fn follow_up(&self, text: impl Into<String>);
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

### 5.3 Session（树形前向兼容的追加日志）

**设计哲学：** 会话是类型化追加日志，每条 entry 含 `parent_id` 前向兼容树结构。v1 仅创建单一线性链（每条 entry 的 parent = 上一条 entry），但所有接口为分支扩展做好准备。

```rust
pub struct SessionEntry {
    pub id:         EntryId,
    pub parent_id:  Option<EntryId>,    // None = root；v1 始终指向链上一项
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
    Custom              { r#type: String, data: serde_json::Value }, // 应用层结构化数据
    // 未来分支扩展点（v1 不创建）：
    // BranchPoint  { from: EntryId },
    // BranchSwitch { to:   EntryId },
}

pub struct CompactionEntry {
    pub summary_message:   AgentMessage,        // CompactionSummaryMessage
    pub first_kept_entry:  EntryId,             // 标记"从此 entry 起的历史仍有效"——下一次 compaction 边界
    pub tokens_before:     usize,
    pub from_hook:         bool,                // 是否由 BeforeCompactHook 提供
    pub details:           Option<serde_json::Value>, // 文件操作等附加数据
}
```

**存储层：**

```rust
pub struct SessionMetadata {
    pub id:           String,
    pub name:         Option<String>,
    pub created_at:   chrono::DateTime<chrono::Utc>,
    pub updated_at:   chrono::DateTime<chrono::Utc>,
    pub model:        Option<String>,
    pub leaf_id:      Option<EntryId>,
}

/// 底层存储 trait：负责字节追加、按 id 查找、leaf 追踪。
pub trait SessionStorage: Send + Sync {
    fn metadata<'a>(&'a self) -> BoxFuture<'a, Result<SessionMetadata, SessionError>>;
    fn create_entry_id(&self) -> EntryId;  // UUIDv7
    fn append_entry<'a>(&'a self, entry: SessionEntry)
        -> BoxFuture<'a, Result<(), SessionError>>;
    fn get_entry<'a>(&'a self, id: EntryId)
        -> BoxFuture<'a, Result<Option<SessionEntry>, SessionError>>;
    fn leaf_id<'a>(&'a self) -> BoxFuture<'a, Result<Option<EntryId>, SessionError>>;
    fn set_leaf_id<'a>(&'a self, id: EntryId)
        -> BoxFuture<'a, Result<(), SessionError>>;
    /// 从 leaf 回溯到 root 的路径（按时间顺序返回，root 在前）
    fn path_to_root<'a>(&'a self, leaf_id: EntryId)
        -> BoxFuture<'a, Result<Vec<SessionEntry>, SessionError>>;
}

/// 仓库抽象——管理多个 session 的生命周期
pub trait SessionRepo: Send + Sync {
    fn create<'a>(&'a self, opts: CreateSessionOptions)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;
    fn open<'a>(&'a self, id: &'a str)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;
    fn list<'a>(&'a self, opts: ListSessionOptions)
        -> BoxFuture<'a, Result<Vec<SessionMetadata>, SessionError>>;
    fn delete<'a>(&'a self, id: &'a str)
        -> BoxFuture<'a, Result<(), SessionError>>;
    /// v1 不实现，但接口保留——分支 fork
    fn fork<'a>(&'a self, source_id: &'a str, from_entry: EntryId)
        -> BoxFuture<'a, Result<Arc<dyn SessionStorage>, SessionError>>;
}

/// 高层 Session 接口：解释 Compaction，构建"当前有效上下文"。
pub struct Session { storage: Arc<dyn SessionStorage> }

impl Session {
    /// 原始路径：从 leaf 回溯到 root 的所有 entry
    pub async fn read_path(&self) -> Result<Vec<SessionEntry>, SessionError>;

    /// 有效上下文：跳过被 Compaction 覆盖的历史，应用 Compaction 摘要，仅返回 messages
    pub async fn build_context(&self) -> Result<Vec<AgentMessage>, SessionError>;

    pub async fn append_message(&self, msg: AgentMessage) -> Result<EntryId, SessionError>;
    pub async fn append(&self, payload: SessionEntryPayload) -> Result<EntryId, SessionError>;
}

pub struct JsonlSessionRepo  { root_dir: PathBuf }
pub struct InMemorySessionRepo { /* ... */ }
```

**v1 范围说明：** 分支创建 / 切换 / fork 实现推迟到 v1.x。Session 的所有接口已为分支做好准备（parent_id、leaf_id、path_to_root），届时只需实现 `BranchPoint` / `BranchSwitch` entry 和 `SessionRepo::fork`。

**超大 entry 处理：** v1 不引入外部存储引用。调用方在 tool 实现中预先截断（参考 pi-agent-core `truncateOutput`）。

### 5.4 Compaction（基于 session entries）

**关键修正：** Compaction 操作的是 **session path entries**，而非纯 message 数组。这样才能：
1. 在路径中定位上次 compaction 的 `first_kept_entry`
2. Cut point 落在 entry 边界，不在 toolResult 中间截断
3. 输出新的 `first_kept_entry` 作为下一次 compaction 边界

```rust
pub struct CompactionSettings {
    pub enabled:           bool,
    pub token_threshold:   usize,           // 超过触发压缩
    pub keep_recent_tokens: usize,           // 保留尾部 N tokens 不压缩
    pub reserve_tokens:    usize,           // 为 LLM 响应预留
    pub summary_model:     String,          // 用便宜模型做摘要
    pub summary_model_info: Option<ModelInfo>,
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
    path:        &[SessionEntry],
    last_compaction: Option<&CompactionEntry>,
    settings:    &CompactionSettings,
) -> Option<CompactionPreparation>;  // None = 无需压缩

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

### 5.6 AgentHarness

编排层：直接驱动 `agent_loop()`（不内嵌 Agent），自己管理 session 写入、配置变更记录、resource 解析。

```rust
pub struct AgentHarness {
    client:       Arc<dyn LlmClient>,
    session:      Session,
    env:          Arc<dyn ExecutionEnv>,
    skills:       Vec<Skill>,
    templates:    Vec<PromptTemplate>,
    state:        Arc<std::sync::Mutex<HarnessState>>,
    event_tx:     tokio::sync::broadcast::Sender<AgentHarnessEvent>,
    hooks:        HarnessHooks,
    stream_options: StreamOptions,
    auth:         Option<Arc<dyn AuthHook>>,
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
}

#[derive(PartialEq, Clone, Copy)]
pub enum HarnessPhase { Idle, Turning, Compacting }

pub struct HarnessHooks {
    pub convert_to_llm:           Arc<dyn ConvertToLlmHook>,
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

## 6. 关键设计决策汇总

| 决策 | 选择 | 理由 |
|---|---|---|
| 事件模型 | 消息级 + token 级双层（含 AgentStart/End、MessageStart/End 携 payload） | 同时支持消息列表 UI 与字符流 UI；Agent↔Harness 通过 AgentEnd payload 传递结果 |
| 消息富类型 | AssistantMessage 携 usage/timestamp/provider/model/error | compaction 估算、回放、错误处理需要 |
| ThinkingContent | 一等公民 ContentBlock variant | Anthropic extended thinking 必需，compaction 时保留思考 |
| Custom message | 框架内置 BranchSummary/CompactionSummary 为具名 variant；其他走 CustomMessage + 必需 ConvertToLlmHook | 类型安全 + 灵活扩展 |
| 工具定义 | `dyn Tool` trait，含 label / prepare_arguments / onUpdate channel / terminate | UI 友好 + LLM 参数兼容 + 流式工具输出 + 自主停止 |
| 工具调度 | 分治：按 Sequential 切子组，组内并发 | 避免连坐降级 |
| 执行环境 | 完整 trait（~13 方法）+ ShellOptions | 对齐 TS 的 FileSystem/Shell；路径操作走 std::path |
| 锁策略 | `std::sync::Mutex` + 快照模式 | 性能 + 跨 await 安全 |
| 阶段锁 | 运行时枚举 | typestate 不适合 async + 长生命周期 |
| Session 结构 | 树形前向兼容（含 parent_id / leaf_id / path_to_root） | v1 单链，分支扩展非破坏性 |
| Session 仓库 | `SessionRepo` trait（create/open/list/delete/fork） | 多 session 管理 |
| Session 读取 | `read_path` 原始 + `build_context` 解释 Compaction | 双层职责 |
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

## 7. 不在范围内（v1）

- **Proxy 模式**（browser → backend 流式转发）：可后续作为独立 feature
- **WASM 目标**：`ExecutionEnv` trait 已抽象；WASM 实现留待后期
- **Session 分支创建/切换/fork**：v1 接口已就绪，实现推迟到 v1.x
- **后台 compaction**：v1 同步触发；v1.x 可考虑异步
- **细粒度权限模型**（capability token）：v1 由 ExecutionEnv 实现方控制
- **超大 entry 外部存储**：v1 调用方截断
- **`prepareArguments` 的 typed schema 泛型**：v1 用 `serde_json::Value`，未来可引入 typed 泛型 Tool
- **Agent loop 以外的框架能力**（规划、记忆管理）：调用方责任
- **典型应用层组件**（如内置 bash tool / file edit tool）：作为示例代码或独立 crate 提供
