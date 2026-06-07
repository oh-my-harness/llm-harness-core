# llm-harness-core 系统设计

**日期：** 2026-06-07  
**状态：** 已批准

## 1. 背景与目标

本项目是 [`@earendil-works/pi-agent-core`](../pi-main/packages/agent) TypeScript 包的 Rust 全量重写。目标不是逐行翻译，而是学习其核心设计哲学，用 Rust 惯用法重新表达。

核心目标：
- 提供完整的 agent 运行时：低层循环、有状态 Agent、编排层 AgentHarness
- 全量对标 pi-agent-core 功能：会话持久化、上下文压缩、skills/templates、执行环境抽象
- 以 [`llm-api-adapter`](https://github.com/hhllhhyyds/llm-api-adapter) 作为 LLM provider 层

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

**依赖：** `serde`, `serde_json`, `futures`（仅 BoxFuture），`tokio-util`（CancellationToken）

**外部类型说明：**
- `LlmClient`：由 `llm-api-adapter` 提供的 trait，代表可发起流式 LLM 调用的客户端
- `CancellationToken`：来自 `tokio-util::sync`，用于跨任务取消传播

### 3.1 消息类型

`ContentBlock` 是 LLM 消息内容的最小单元，对应 Anthropic/OpenAI 的 content block 模型：

```rust
pub enum ContentBlock {
    Text  { text: String },
    Image { media_type: String, data: String },  // base64
    ToolUse { id: String, name: String, input: serde_json::Value },
}
```

`EntryId` 是 session log 中每条 entry 的唯一标识，使用 UUIDv7（时间有序）：

```rust
pub struct EntryId(pub [u8; 16]);  // UUIDv7
```

```rust
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    Custom(CustomMessage),       // 应用层自定义消息
}

pub struct UserMessage    { pub content: Vec<ContentBlock> }
pub struct AssistantMessage { pub content: Vec<ContentBlock>, pub stop_reason: Option<StopReason> }
pub struct ToolResultMessage { pub tool_use_id: String, pub content: Vec<ContentBlock>, pub is_error: bool }
pub struct CustomMessage  { pub r#type: String, pub data: serde_json::Value }
```

### 3.2 事件类型

Agent 的行为通过事件流暴露，调用方"观测"事件而非被回调驱动。

```rust
pub enum AgentEvent {
    TurnStart     { index: u32 },
    TurnEnd       { index: u32 },
    TextDelta     { text: String },
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, partial_input: String },
    ToolCallEnd   { id: String, result: ToolResult },
    Error(AgentError),
    Done,
}
```

### 3.3 Tool trait

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

### 3.4 ExecutionEnv trait

TS 版将执行环境视为 Node.js 专属，在后期才抽象为 trait。本设计从第一天起即为 trait，支持 OS、WASM、测试 mock 等不同实现。

```rust
pub trait ExecutionEnv: Send + Sync {
    fn read_file<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<String>>;
    fn write_file<'a>(&'a self, path: &'a Path, content: &'a str) -> BoxFuture<'a, Result<()>>;
    fn list_dir<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<Vec<FileInfo>>>;
    fn execute_shell<'a>(&'a self, cmd: &'a str, abort: CancellationToken)
        -> BoxFuture<'a, Result<ShellOutput>>;
    fn working_dir(&self) -> &Path;
}

pub struct ShellOutput { pub stdout: String, pub stderr: String, pub exit_code: i32 }
pub struct FileInfo    { pub path: PathBuf, pub is_dir: bool, pub size: u64 }
```

### 3.5 其他基础类型

```rust
pub enum ThinkingLevel { Off, Minimal, Low, Medium, High, XHigh }
pub struct AgentContext { pub system_prompt: Option<String>, pub messages: Vec<AgentMessage> }
pub struct ToolResult   { pub content: Vec<ContentBlock>, pub details: serde_json::Value }
```

## 4. `llm-harness-loop`

**职责：** 纯函数式 agent loop——给定上下文与配置，返回事件流。不持有状态，不管理会话。

**依赖：** `llm-harness-types`, `llm-api-adapter`, `tokio`, `tokio-stream`, `futures`

### 4.1 API

```rust
pub struct LoopConfig {
    pub tools:          Vec<Arc<dyn Tool>>,
    pub execution_mode: ToolExecutionMode,

    /// 每次 LLM 调用前转换上下文（compaction 通过此钩子挂入，与 loop 解耦）
    pub transform_context: Option<Arc<dyn TransformContextHook>>,

    /// Tool call 前后拦截（权限控制、审计、mock）
    pub before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    pub after_tool_call:  Option<Arc<dyn AfterToolCallHook>>,

    /// 响应式注入：turn 结束后注入消息（steer），agent 停止后注入（follow_up）
    pub steer_rx:     Option<SteerReceiver>,
    pub follow_up_rx: Option<FollowUpReceiver>,

    /// 调用方决定是否继续下一轮
    pub should_stop: Option<Arc<dyn ShouldStopHook>>,
}

pub fn agent_loop(
    client: Arc<dyn LlmClient>,
    ctx:    AgentContext,
    config: LoopConfig,
) -> impl Stream<Item = AgentEvent> + Send
```

### 4.2 Tool batch 语义

每轮 LLM 返回的 tool call 集合按以下规则执行：
- 若所有 tool 均为 `Parallel`：`join_all` 并发执行
- 若任意一个 tool 为 `Sequential`：整批退化为顺序执行

此语义直接映射到 Rust 的 `join_all` vs 顺序循环，无需额外设计。

### 4.3 Steering vs Follow-up

- **Steer**：在一轮 tool 执行结束、下一次 LLM 调用之前注入用户消息，影响当前轮次的后续走向
- **Follow-up**：在 agent 自然停止后注入，触发新的一轮执行

两者均通过 channel 实现（`mpsc::Receiver`），loop 在合适的点轮询 channel。

### 4.4 事件结算

TS 版 `Agent` 类需要 await 所有 `agent_end` 监听器（因 JS 事件监听器默认 fire-and-forget）。Rust 的 `broadcast::Receiver` 是拉取式，调用方直接 `await` 自己的 receiver 即可，框架层无需做额外保证。

## 5. `llm-harness`

**职责：** 在 loop 之上提供有状态 Agent、编排层 AgentHarness、会话持久化、上下文压缩、skills/templates 加载。

**依赖：** `llm-harness-types`, `llm-harness-loop`, `tokio`, `serde_json`, `serde_yaml`（frontmatter）

### 5.1 Agent

有状态包装器，在 `agent_loop` Stream 之上提供订阅、阶段控制、响应式注入。

```rust
pub struct Agent {
    client:       Arc<dyn LlmClient>,
    state:        Arc<Mutex<AgentState>>,
    event_tx:     broadcast::Sender<AgentEvent>,
    steer_tx:     mpsc::Sender<String>,
    follow_up_tx: mpsc::Sender<String>,
    abort:        CancellationToken,
}

struct AgentState {
    phase:          AgentPhase,   // Idle | Running
    model:          String,
    thinking_level: ThinkingLevel,
    tools:          Vec<Arc<dyn Tool>>,
    messages:       Vec<AgentMessage>,
    system_prompt:  Option<String>,
}
```

方法按阶段分类：

```rust
// 结构性操作：仅 Idle 阶段可调用，否则返回 Err(NotIdle)
pub async fn prompt(&self, text: impl Into<String>) -> Result<()>;

// 队列操作：任何阶段均安全
pub fn steer(&self, text: impl Into<String>);
pub fn follow_up(&self, text: impl Into<String>);
pub fn abort(&self);

// 观测
pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;
pub async fn wait_for_idle(&self);
```

### 5.2 Turn 快照

**设计哲学（来自 pi-agent-core）：** 每次 turn 开始时，将当前 model、tools、系统提示做一次不可变快照。turn 进行中对这些配置的修改不影响当前 turn，仅影响下一轮。

```rust
#[derive(Clone)]
struct TurnSnapshot {
    model:          String,
    thinking_level: ThinkingLevel,
    tools:          Vec<Arc<dyn Tool>>,
    system_prompt:  Option<String>,
}
// turn 开始时：let snapshot = state.lock().into_snapshot();
```

### 5.3 Session

**设计哲学：** 会话是一条**不可变的类型化追加日志**，而非 messages 数组。支持分支、回放、审计；compaction 不删除历史，而是追加一条 Compaction entry。

```rust
pub enum SessionEntry {
    Message(MessageEntry),
    ModelChange      { to: String },
    ThinkingLevelChange { to: ThinkingLevel },
    ToolsChange      { active: Vec<String> },
    Compaction(CompactionEntry),       // 压缩摘要替换旧消息范围
    BranchSummary(BranchSummaryEntry), // 分支导航摘要
    Label            { name: String }, // 给当前位置命名，便于导航
    Leaf,                              // 标记当前活跃分支末端
}

pub trait SessionStorage: Send + Sync {
    fn append<'a>(&'a self, entry: SessionEntry)
        -> BoxFuture<'a, Result<EntryId>>;
    fn read_range<'a>(&'a self, from: Option<EntryId>)
        -> BoxFuture<'a, Result<Vec<(EntryId, SessionEntry)>>>;
}

pub struct JsonlSessionStorage  { path: PathBuf }
pub struct InMemorySessionStorage { entries: Arc<Mutex<Vec<(EntryId, SessionEntry)>>> }
```

### 5.4 Compaction

与 loop 层解耦，通过 `transform_context` 钩子挂入 AgentHarness：

```rust
pub struct CompactionSettings {
    pub token_threshold: usize,  // 超过此 token 数触发压缩
    pub summary_model:  String,  // 可用比主模型更便宜的模型做摘要
    pub retain_recent:  usize,   // 压缩时保留最近 N 轮不压缩
}

pub async fn compact(
    client:   &dyn LlmClient,
    messages: &[AgentMessage],
    settings: &CompactionSettings,
) -> Result<CompactionResult>

pub struct CompactionResult {
    pub retained:       Vec<AgentMessage>, // 保留的消息（含摘要）
    pub summary:        String,
    pub compressed_ids: Vec<MessageId>,    // 被替换的消息 ID，写入 session log
}
```

### 5.5 Skills 与 PromptTemplates

```rust
pub struct Skill {
    pub name:        String,
    pub description: String,
    pub content:     String,
    pub source:      PathBuf,
}

pub struct PromptTemplate {
    pub name:     String,
    pub content:  String,  // 含 {{placeholder}} 占位符
    pub source:   PathBuf,
}

// 递归扫描目录，解析 YAML frontmatter，忽略 .gitignore 规则
pub async fn load_skills(
    env:  &dyn ExecutionEnv,
    dirs: &[PathBuf],
) -> (Vec<Skill>, Vec<SkillDiagnostic>)

pub fn format_skill_for_system_prompt(skills: &[Skill]) -> String
pub fn invoke_template(template: &PromptTemplate, args: &HashMap<String, String>) -> String
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
    phase:     HarnessPhase,
    hooks:     HarnessHooks,
}

#[derive(PartialEq)]
enum HarnessPhase { Idle, Turning, Compacting }

pub struct HarnessHooks {
    pub before_turn:     Option<Arc<dyn BeforeTurnHook>>,
    pub after_turn:      Option<Arc<dyn AfterTurnHook>>,
    pub before_tool_call: Option<Arc<dyn BeforeToolCallHook>>,
    pub after_tool_call:  Option<Arc<dyn AfterToolCallHook>>,
}
```

操作分类：

```rust
// 结构性操作：仅 Idle 阶段，否则 Err(HarnessError::NotIdle)
pub async fn prompt(&mut self, text: impl Into<String>) -> Result<()>;
pub async fn compact(&mut self) -> Result<()>;
pub async fn reload_skills(&mut self) -> Result<()>;
pub async fn prompt_from_template(&mut self, name: &str, args: HashMap<String, String>) -> Result<()>;

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
| 事件流机制 | `Stream<Item=AgentEvent>` + `broadcast::Receiver` | Rust 拉取式，无需框架层结算 |
| 工具定义 | `dyn Tool` trait object | 调用方可自由实现，无需依赖 typebox |
| 执行环境 | `dyn ExecutionEnv` trait，第一天即抽象 | 支持 OS/WASM/mock，优于 TS 版后期抽象 |
| 阶段锁 | 运行时枚举 `HarnessPhase` | typestate 在 async + 长生命周期场景下过于繁琐 |
| 会话模型 | 追加日志 + `SessionEntry` enum | 不可变、可分支、支持压缩溯源 |
| 压缩解耦 | 通过 `transform_context` 钩子挂入 loop | 压缩策略不与循环逻辑耦合 |
| Tool 参数 | `serde_json::Value`（JSON Schema） | 灵活，后期可引入 typed 泛型工具 |
| Crate 拆分 | workspace + 3 crates | 关注点分离，允许按层依赖 |

## 7. 不在范围内

- Proxy 模式（browser → backend 流式转发）：暂不实现，可后续作为独立 feature
- WASM 目标：`ExecutionEnv` trait 已预留扩展点，WASM 实现留待后期
- Agent loop 以外的 agent 框架能力（规划、记忆管理）：属于调用方责任
