## 3. `llm-harness-types`

**职责：** 零 IO 的纯类型层。所有跨 crate 共享的类型和 trait 均在此定义。

**依赖：** `serde`, `serde_json`, `futures`（`BoxFuture`），`tokio`（feature = `sync`，用于 `mpsc::Sender`），`tokio-util`（`CancellationToken`），`thiserror`，`uuid`，`chrono`

> **为什么是 "零 IO"？** 纯类型层不执行任何 I/O 操作——不读文件、不发网络请求、不启动线程。这带来三个好处：(1) 编译极快，依赖树最浅；(2) 类型可在任何环境（WASM、嵌入式）中引用而不受平台限制；(3) 所有依赖 crate 都可以 confidence 地依赖 types，不会意外引入重量级依赖。

**外部类型说明：**
- `LlmClient`：由 `llm-api-adapter` 提供的 trait，代表可发起流式 LLM 调用的客户端。types 不定义它——只引用其概念。实际定义在 `llm-api-adapter`，由 `llm-harness-loop` 通过 `pub use` 重导出。
- `ModelInfo`：由 `llm-api-adapter` 提供，含 `provider`、`api`、`model_id`、`context_window`、`max_tokens`、`cost` 等元数据；compaction token 估算依赖此结构。
- `CancellationToken`：来自 `tokio-util::sync`，用于跨任务取消传播。选择 `tokio-util` 而非自己造轮子，因为它是 tokio 生态的标准取消原语，已经被广泛审计。

---

### 3.1 基础标识与错误类型

> **本节设计目标：** 为整个框架提供统一的标识符（EntryId）和错误类型体系。所有错误类型使用 `thiserror` derive——这是 Rust 生态的惯例，自动生成 `Display` + `std::error::Error` 实现，且支持 `?` 运算符和 `#[from]` 自动转换。

---

#### EntryId —— 通用标识符

```rust
/// Session log 中每条 entry 的唯一标识，UUIDv7（时间有序）。
/// 兼任消息标识：消息即 SessionEntry::Message，其 EntryId 即消息 ID。
/// 必须实现 Display/FromStr 以便 JSONL 序列化和跨进程引用。
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub struct EntryId(pub uuid::Uuid);
// impl Display, FromStr, Serialize, Deserialize (字符串形式)
```

> **为什么是 UUIDv7？** UUIDv7 以 Unix 毫秒时间戳为前缀，后面跟随机位。相比 UUIDv4（纯随机），v7 的两个优势：(1) 时间有序——按时间排序 entry 时直接比较 UUID 字节即可，不需要额外查 timestamp 字段；(2) 数据库友好——B-tree 索引在时间有序的主键上性能远好于随机主键。相比自增 ID，v7 支持分布式生成无需协调。
>
> **为什么 newtype 而不是 type alias？** `EntryId(pub uuid::Uuid)` 而非 `type EntryId = uuid::Uuid`。newtype 提供类型安全——防止误将 EntryId 当作普通 UUID 使用，也防止将其他 UUID（如 session ID）误传给接受 EntryId 的函数。同时 inner 字段 `pub` 允许 workspace 内 crate 直接解构，避免过多的构造器样板。
>
> **为什么 Display/FromStr 必须实现？** JSONL 文件中 entry ID 以字符串形式出现；session fork 时也需要字符串形式的 ID 作为引用。"跨进程引用"指的是不同进程（如浏览器→server proxy）通过字符串传递 entry ID。

---

#### ToolError —— 工具执行错误

```rust
/// Tool 执行失败。
#[derive(thiserror::Error, Debug)]
pub enum ToolError {
    #[error("invalid arguments: {0}")] InvalidArguments(String),
    #[error("tool aborted")] Aborted,
    #[error("tool execution failed: {0}")] Execution(String),
    #[error(transparent)] Other(#[from] anyhow::Error),
}
```

> **四种变体的设计理由：**
> - `InvalidArguments`：Tool 在 schema 校验或 `prepare_arguments` 阶段发现参数不合法。与执行失败区分——调用方可以仅重试（LLM 可能修正参数）而不是放弃。
> - `Aborted`：用户通过 `CancellationToken` 取消了操作。与一般执行失败区分——这不是 tool 的错，UI 不应显示为 "错误"。
> - `Execution`：Tool 在执行过程中失败（shell 命令非零退出、网络超时等）。模板化的 `String` 而非结构化错误——因为 tool 的种类无限，无法预定义所有失败模式。`String` 保留灵活性。
> - `Other`：兜底变体，通过 `#[from] anyhow::Error` 支持任意 `std::error::Error` 的 `?` 转换。`#[error(transparent)]` 保留原始错误的 Display 和 source chain。
>
> **为什么没有 `Success` 变体？** Rust 惯例是 `Result<T, E>`——成功走 `Ok(ToolResult)`，失败走 `Err(ToolError)`。不需要在 error 类型中编码成功。

---

#### AgentError —— Agent 运行时错误

```rust
#[derive(thiserror::Error, Debug, Clone)]
pub enum AgentError {
    #[error("llm provider error: {0}")] Provider(String),
    #[error("tool error: {tool_name}: {message}")] Tool { tool_name: String, message: String },
    #[error("aborted")] Aborted,
    #[error("agent is not idle")] NotIdle,
    #[error("internal: {0}")] Internal(String),
}
```

> **五种变体的设计理由：**
> - `Provider`：LLM provider 返回错误（API key 无效、rate limit、模型不存在等）。`String` 而非结构化——不同 provider 的错误格式不同，且我们只做透传。
> - `Tool`：携带 `tool_name`（哪个 tool 失败）和 `message`（失败原因）。与 `ToolError` 不同——这是从 Agent 视角看到的 tool 错误，已经丢失了 `ToolError` 的变体信息（因为 tool 执行发生在 loop 内部，loop 将 `ToolError` 转为 `AgentError::Tool`）。
> - `Aborted`：用户取消。与 `ToolError::Aborted` 平行的概念，但在 Agent 层级。
> - `NotIdle`：调用方在 Agent 处于 `Running` 阶段时尝试调用 `prompt()` 等结构性操作。这不是 bug——是正常的并发控制。
> - `Internal`：框架内部错误（锁中毒、channel 断开等）。兜底。
>
> **为什么 `Clone`？** AgentError 需要通过 `broadcast::Sender` 发送给多个订阅者。`broadcast` 要求 `Clone`。`Provider` 和 `Internal` 的 `String` 已经满足 Clone；`Tool` 的字段也是 Clone。

---

#### StopReason —— LLM 停止原因

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason { EndTurn, MaxTokens, StopSequence, ToolUse, Other }
```

> **为什么需要这个类型？** LLM API 返回的 `finish_reason` / `stop_reason` 需要被解析为 Rust 枚举以便 match。这五个变体覆盖了主流 LLM provider 的停止原因：
> - `EndTurn`：模型自然结束（"stop"）
> - `MaxTokens`：达到 `max_tokens` 限制被截断
> - `StopSequence`：匹配到自定义停止序列
> - `ToolUse`：模型请求调用工具
> - `Other`：未知/未分类的停止原因
>
> **为什么 `Copy`？** 停止原因是轻量标签——4 字节的枚举，不需要所有权语义。`Copy` 允许在 match 分支和闭包中自由传递。

---

#### EnvError —— 执行环境错误

```rust
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
```

> **十种变体的设计理由：**
> - 前五种（`NotFound` 到 `IsADirectory`）携带 `PathBuf`——调用方需要知道哪个路径出了问题，以便向用户展示或重试。
> - `Aborted`：操作被 `CancellationToken` 取消。与文件系统错误区分——这不是 I/O 问题。
> - `InvalidUtf8`：文件内容不是有效的 UTF-8（`read_text_file` 的预期行为）。`PathBuf` 标识出问题的文件。
> - `ShellFailed`：shell 命令执行失败（非零退出码）。携带 `exit_code` 和 `stderr`——调用方通常需要 stderr 内容来诊断问题。
> - `Io`：标准库 I/O 错误的透明包装。`#[from]` 允许 `?` 运算符从 `std::io::Error` 自动转换。
> - `Other`：兜底——用于自定义 `ExecutionEnv` 实现报告框架未分类的错误。
>
> **为什么不用 `std::io::Error` 替代全部变体？** `std::io::Error` 是 POSIX 风格的错误码（`ErrorKind`），无法表达 "shell 非零退出"、"UTF-8 无效" 这种语义。独立的 `EnvError` 让调用方可以按语义 match 而非按 OS 错误码。

---

#### SessionError —— Session 操作错误

```rust
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
```

> **八种变体的设计理由：**
> - `EntryNotFound`：通过 ID 查找 entry 失败。携带 `EntryId` 便于错误日志关联。
> - `SessionNotFound` / `SessionAlreadyExists`：SessionRepo 操作的前置条件检查。携带 session ID 字符串。
> - `NotALeaf`：操作要求目标为 leaf（如 `delete_branch`）但目标有子节点。
> - `InvalidParent`：`append_entry` 时 parent_id 指向不存在的 entry——数据损坏或并发竞争。
> - `Io`：底层存储 I/O 错误的透传。
> - `Serialization`：JSONL 反序列化失败（文件损坏或格式不兼容）。
> - `ConcurrentModification`：乐观并发控制检测到冲突——两个写入者同时修改同一路径。
>
> **为什么不合并到 `EnvError`？** Session 是独立子系统——它有自己独特的错误语义（entry not found、not a leaf）。合并会迫使调用方在 session 相关代码中处理无关的 `EnvError` 变体。

---

#### CompactionError —— 压缩错误

```rust
#[derive(thiserror::Error, Debug)]
pub enum CompactionError {
    #[error("not enough tokens to compact")]      InsufficientTokens,
    #[error("summary model call failed: {0}")]    SummaryFailed(String),
    #[error(transparent)]                          Session(#[from] SessionError),
    #[error(transparent)]                          Agent(#[from] AgentError),
}
```

> **四种变体的设计理由：** Compaction 的操作跨越两个域——Session（读写 entries）和 LLM 调用（生成摘要）。所以它的错误类型也跨越这两个域：
> - `InsufficientTokens`：`reserve_tokens` 设置过大，导致没有足够的 token 做摘要请求本身。
> - `SummaryFailed`：调用摘要模型失败（provider 错误、超时等）。
> - `Session` / `Agent`：通过 `#[transparent]` + `#[from]` 自动从子系统的错误转换，调用方可以用 `?` 自然传播。

---

#### TemplateError —— 模板错误

```rust
#[derive(thiserror::Error, Debug)]
pub enum TemplateError {
    #[error("template not found: {0}")]                    NotFound(String),
    #[error("missing required argument at position {0}")]  MissingArg(usize),
    #[error("invalid argument syntax: {0}")]               InvalidSyntax(String),
}
```

> **设计理由：** PromptTemplate 的占位符替换（`$1`, `$2`, `$@` 等）可能在运行时因参数不足或语法错误而失败。三种变体覆盖了：模板查找失败、参数缺失、参数语法错误。

---

#### HarnessError —— AgentHarness 顶层错误

```rust
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
```

> **设计理由：** `HarnessError` 是 AgentHarness 对外暴露的统一错误类型——调用方只需要处理这一个错误类型，而不是六种不同的子系统错误。`#[from]` + `#[transparent]` 使得所有子系统的错误可以通过 `?` 自动转换为 `HarnessError`。
>
> **为什么 `NotIdle` 携带 `HarnessPhase`？** 调用方在错误时想知道 "当前是什么阶段"——可能有助于诊断（例如调用方不知道上一个操作还没结束）。
>
> **`HarnessPhase` 为什么在 types？** `HarnessError::NotIdle(HarnessPhase)` 需要 HarnessPhase 在同一 crate 中。AgentPhase 留在 harness crate，因为 `AgentError::NotIdle` 是单元变体（不需要携带 phase 信息）。

---

#### DiagnosticLevel

```rust
#[derive(Debug, Clone, Copy)]
pub enum DiagnosticLevel { Warn, Error }
```

> **设计理由：** Skills 和 PromptTemplates 加载过程可能产生诊断信息。`Warn` 表示可以继续（如单个 skill 文件格式不合法），`Error` 表示整体操作失败。仅两个级别——不需要 `Info` 和 `Debug` 因为正常操作不应产生诊断输出。

---

#### HarnessPhase

```rust
/// Harness 运行阶段——提升到 types 是因为 HarnessError::NotIdle 需要携带它。
/// AgentPhase 留在 llm-harness 中（AgentError::NotIdle 无 payload）。
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum HarnessPhase { Idle, Turning, Compacting, Branching }
```

> **四个阶段的设计理由：**
> - `Idle`：没有正在进行的操作，接受 `prompt()`、`compact()`、分支操作。
> - `Turning`：正在执行 agent loop（LLM 调用 + tool 执行）。
> - `Compacting`：正在执行 compaction（调用摘要模型，写入 Compaction entry）。
> - `Branching`：正在执行分支导航（`navigate_tree`，含可选的 branch summary 生成）。
>
> **为什么 Compacting 和 Branching 是独立阶段而非复用 Turning？** 三个原因：(1) Turning 期间可以 `steer()`/`follow_up()`，但 Compacting/Branching 期间不可以——它们是原子操作；(2) Compacting/Branching 使用的 LLM 客户端是独立调用（不经过 agent_loop）；(3) 不同阶段对应不同的错误恢复策略。
>
> **为什么在 types crate 而不是 harness？** 纯技术原因——`HarnessError::NotIdle` 需要携带 `HarnessPhase`，而 `HarnessError` 在 types 中。如果将 HarnessPhase 留在 harness，则 types 依赖 harness（反向依赖），违反依赖拓扑。

---

### 3.2 ContentBlock 与消息类型

> **本节设计目标：** 定义 Agent 内部使用的消息模型。"内部"意味着这些消息会进入 session log、被 compaction 处理、被 convert_to_llm 转换。它们比 LLM API 的原始 Message 更丰富——携带 timestamp、usage、error 等元数据，支撑回放、审计、compaction 等功能。

---

#### ContentBlock —— 消息内容最小单元

```rust
pub enum ContentBlock {
    Text     { text: String },
    Thinking { thinking: String, signature: Option<String> },  // provider 推理内容（Anthropic/OpenAI/DeepSeek 等均支持）
    Image    { source: ImageSource },
    ToolUse  { id: String, name: String, input: serde_json::Value },
}

pub enum ImageSource {
    Base64 { media_type: String, data: String },
    // 未来扩展：Url { url: String }, Id { id: String }
}
```

> **设计理由：**
>
> **`Thinking` 为什么是一等公民？** Anthropic、OpenAI（o 系列）、DeepSeek（R 系列）等主流 provider 均支持推理/思考内容。模型在生成最终回复前进行内部推理，thinking 内容块需要被 (1) 保留在 session log 中以备审计；(2) 在 compaction 时保留思考痕迹（思考内容包含重要的决策上下文）；(3) 在消息历史中传递（部分 provider API 要求在后续请求中传回 thinking block）。缺了它，多轮对话中的推理会断裂。
>
> **`signature: Option<String>`** 是 Anthropic 特有的 content signature（用于验证 thinking 内容完整性）。其他 provider 不使用此字段，置 `None` 即可。
>
> **`Image` 的 `ImageSource` 为什么用 enum 而非只有 Base64？** Base64 内联是最通用的方式（不依赖外部 URL），但膨胀 33%，且无法利用 API 的图片缓存。预留 `Url` 和 `Id` 变体是为了后续支持更高效的图片引用，无需破坏现有 API。
>
> **`ToolUse` 中的 `id`** 是 LLM 为每个 tool call 分配的唯一标识——后续的 `ToolResultMessage` 通过 `tool_use_id` 匹配到对应的 `ToolUse`。这是一个标准的 LLM tool calling 协议字段。

---

#### AgentMessage —— 消息联合体

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
```

> **设计理由：**
>
> **六种变体，三个类别：**
> 1. **标准 LLM 消息**（`User`, `Assistant`, `ToolResult`）：直接对应 LLM API 的消息角色。这些由 agent loop 产生和消费。
> 2. **框架摘要消息**（`BranchSummary`, `CompactionSummary`）：框架内部生成的特殊消息类型——它们不是 LLM 返回的，而是框架在 compaction 或分支导航时插入的。拥有独立的变体使 session log 具有自描述性——你可以只看 entry 类型就知道 "这是一次 compaction"。
> 3. **应用层扩展**（`Custom`）：允许应用层插入任意类型的消息（如 "用户打开了文件 X"、"部署完成"）。这些消息不直接进入 LLM 上下文——必须由 `ConvertToLlmHook` 显式转换。
>
> **为什么 BranchSummary 和 CompactionSummary 是具名变体而非 Custom？** 因为框架需要理解它们的语义：(1) compaction 逻辑需要识别 `CompactionSummary` 来计算 token 估算；(2) `convert_to_llm` 的默认实现需要将它们转为带特殊前缀的 UserMessage；(3) session log 的类型化查询（`find_entries_by_type`）需要区分它们。如果它们是 Custom，框架就无法提供默认行为。

---

#### 具体消息结构体

```rust
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

> **设计理由：**
>
> **为什么每个消息都有 `timestamp`？** Session log 的 entry 是按时间排序的，但消息可能被 compaction 重新排列或被应用层筛选——`timestamp` 保留了消息被创建时的原始时间。另外，在跨 session fork 时，timestamp 是保持原始时间线的关键。
>
> **`AssistantMessage` 字段的功能性依赖：**
> - `stop_reason`：Agent loop 用它判断是否继续——`ToolUse` 意味着需要执行工具，`EndTurn` 意味着可以停止。
> - `provider` / `api` / `model`：session 回放时需要知道 "这条消息是哪个模型生成的"——因为后续 compaction 可能用不同的摘要模型。
> - `usage`：compaction 的 token 估算**直接依赖**最近一次 assistant 消息的 `usage.totalTokens`。没有这个字段，compaction 只能靠字符估算（误差大）。
> - `error_message`：LLM 返回错误时（stop_reason = error/aborted），错误文本保存在这里。Agent 的 `error_message` 字段就是从这个快照来的。
>
> **`ToolResultMessage` 的 `is_error: bool`** 而非 `Result`：tool 执行失败不是 Rust 级别的 `Err`——它仍然是有效消息，需要发送给 LLM 让它知道 "这个 tool 调用失败了，请修正"。用 `bool` 而非 `Result` 避免了将 ToolError 嵌入消息类型（保持消息类型的序列化友好）。
>
> **`TokenUsage` 的字段选择：** `input_tokens` (prompt)，`output_tokens` (completion)，`cache_read_tokens` (Anthropic prompt caching 命中的 token)，`cache_creation_tokens` (新写入缓存的 token)。这四个字段覆盖了 Anthropic 和 OpenAI 的 usage 模型。用 `u32` 而非 `u64`——单次 LLM 调用的 token 数不可能超过 2^32（约 40 亿）。
>
> **`CustomMessage` 的两个字段：** `r#type` 是应用层自由定义的类别标签（如 `"artifact"`、`"notification"`）；`data` 是任意 JSON 负载。这种设计允许应用层通过同一个 Custom 变体表达无穷多种消息类型，而无需修改 AgentMessage enum。

---

### 3.3 事件模型（消息级 + token 级双层）

> **本节设计目标：** Agent 的行为通过事件流暴露。事件流是 Agent 与外部世界（UI、AgentHarness）的唯一通信渠道。双层设计（消息级 + token 级）是为了同时支持两种 UI 模式：消息列表 UI（看到完整的消息边界）和字符流 UI（看到逐字输出）。

---

#### AgentEvent —— 完整事件枚举

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
    /// message_id 由 loop 从 StreamEvent::MessageStart.id 提取。
    /// 不携带 provider/model/api——这些字段在 MessageEnd 的完整 AssistantMessage 中。
    MessageStart  { message_id: String },
    /// 流式期间，partial assistant message 的当前快照（每次更新覆盖之前的）
    MessageUpdate { message_id: String, partial: AssistantMessage },
    /// 消息完整生成完毕，含 stop_reason 和 usage
    MessageEnd    { message_id: String, message: AssistantMessage },

    // === Token 级（字符流） ===
    TextDelta        { message_id: String, text: String },
    ThinkingDelta    { message_id: String, thinking: String, signature: Option<String> },
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

> **设计理由（按事件类别）：**
>
> **Agent 生命周期（AgentStart / AgentEnd）：**
> - `AgentStart` 携带 `initial_messages`——调用方可以知道这次运行注入了哪些初始消息（用户 prompt、queued next_turn 等）。
> - `AgentEnd` 携带 `new_messages`——这是 Agent ↔ Harness 的**关键接口契约**。AgentHarness 依赖 `AgentEnd` 中的完整消息列表来写入 session log。即使广播 channel 丢失了中间的 `MessageEnd` 事件，Harness 仍能从 `AgentEnd` 获取完整结果。
>
> **Turn 生命周期（TurnStart / TurnEnd）：**
> - `TurnStart { index }`：turn 编号从 0 开始递增。调用方可用它追踪 "第几轮对话"。
> - `TurnEnd` 携带本轮完整的 assistant message 和所有 tool 执行结果。tool_results 的 `(String, Result<ToolResult, ToolError>)` 中 String 是 `tool_use_id`，Result 区分成功和失败。Harness 利用 TurnEnd 做 save point（flush pending writes）。
>
> **消息级事件（MessageStart / MessageUpdate / MessageEnd）：**
> - 这三个事件描绘了一条 assistant message 的完整生命周期。`MessageUpdate` 中的 `partial` 是消息的当前快照——每次更新覆盖前一次。这允许 UI 渲染 "正在生成的消息" 而无需自己拼接 TextDelta。
> - `message_id` 是 loop 内部分配的标识（不是 EntryId——此时消息还未进入 session）。它关联同一消息的 token 级事件。
>
> **Token 级事件（TextDelta / ThinkingDelta / ToolCall*）：**
> - 这些事件为**字符流 UI** 提供原始增量。调用方可以逐字渲染，无需解析完整的消息结构。
> - `ToolCallStart` / `ToolCallArgsDelta` / `ToolCallEnd`：LLM 的 tool call 参数是分块到达的 JSON。`ToolCallArgsDelta` 提供原始 JSON 片段，`ToolCallEnd` 提供解析后的完整参数。
> - `ToolCall*`（LLM 请求工具）与 `ToolExecution*`（Rust 执行工具）被严格区分——前者是 LLM 的输出，后者是框架的行为。
>
> **工具执行事件（ToolExecution*）：**
> - `ToolExecutionStart`：tool 开始执行，携带 `tool_use_id`、`tool_name`、`args`。
> - `ToolExecutionUpdate`：长时间运行的 tool 通过 `ToolContext.update_tx` 推送中间结果，loop 转发为此事件。
> - `ToolExecutionEnd`：tool 执行完毕，携带 `Result<ToolResult, ToolError>`。注意这里的 Result 是 tool 执行的 Rust 结果——不是发给 LLM 的消息。LLM 看到的是 `ToolResultMessage`（由 Harness 在 `ToolExecutionEnd` 后构造并写入 session）。
>
> **Error 事件：** 携带 `AgentError`——表示 loop 遇到了不可恢复的错误（provider 持续失败、内部 channel 断开等）。收到此事件后，调用方应预期 `AgentEnd` 立即到达。
>
> **为什么没有 `AgentEvent::Done`？** v1 设计中 `AgentEnd` 替代了 Done——AgentEnd 就是 "done"，并且还携带结果。不需要一个不带 payload 的终止事件。

---

**事件传递语义：**

> **为什么用 `broadcast` 而非 `watch` 或 `mpsc`？**
> - `broadcast`：一份发送，多个订阅者各自接收。适合 UI 场景（多个组件订阅同一事件流）。容量有界（默认 256），慢消费者丢失事件——这是有意选择：事件流是 "当前状态的通知"，不是 "可靠的消息队列"。需要可靠性的调用方应依赖 `MessageEnd`/`AgentEnd` 的完整 payload，或通过 session log 重建状态。
> - 如果使用 `watch`：每个订阅者只看到最新值，旧值被覆盖——中间事件（如 `ToolExecutionStart`）可能丢失。
> - 如果使用 `mpsc`：每个订阅者需要独立 channel，Agent 需要维护订阅者列表——复杂度高且无法支持动态订阅。
>
> **"慢消费者丢失事件"是特性而非 bug：** 事件流用于 UI 渲染——如果 UI 跟不上 agent 的速度，丢弃旧的 TextDelta 而保留最新的 MessageUpdate 是合理的行为。状态重建应该走 Session log，不走事件流。

---

### 3.4 Tool trait

> **本节设计目标：** 定义工具（Tool）的接口——框架调用工具的唯一方式。Tool trait 是 `dyn Trait`（trait object）——允许调用方在运行时注册不同类型的工具，框架不关心工具的具体实现。

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
    pub env:               Arc<dyn ExecutionEnv>,
    pub abort:             CancellationToken,
    /// 当前 tool call 在 LLM 返回中的 id（用于事件关联）
    pub tool_use_id:       String,
    /// 当前轮次索引（从 0 开始）。hook 可用于 "前 N 轮不需要确认" 之类的策略。
    pub turn_index:        u32,
    /// 触发本次 tool call 的完整 LLM 响应。
    /// loop 在 MessageEnd 后持有完整 AssistantMessage，构造 ToolContext 时传入。
    /// 同一 AssistantMessage 可能触发多个 tool call，通过 Arc 共享。
    pub assistant_message: Arc<AssistantMessage>,
    /// 长时间运行的 tool 可通过此 channel 推送部分结果。
    /// 接收端转发为 AgentEvent::ToolExecutionUpdate。
    pub update_tx:         tokio::sync::mpsc::Sender<ToolResult>,
}

pub struct ToolResult {
    pub content: Vec<ContentBlock>,
    pub details: serde_json::Value,
    /// 当一个 batch 中所有 tool 都返回 terminate=true 时，agent 提前停止 loop。
    /// 允许 tool 自主宣告"任务完成"。
    pub terminate: bool,
}
```

> **设计理由：**
>
> **`Send + Sync`：** Tool 实例会被多个线程共享（loop 任务、事件处理任务），必须是线程安全的。
>
> **`name` vs `label`：** `name` 是稳定的程序标识符（如 `"read_file"`），在 session log 和 LLM tool definition 中使用。`label` 是人类可读的 UI 显示名（如 `"Read File"`）。默认 `label()` 回退到 `name()`——大多数简单工具不需要区分两者。
>
> **`prepare_arguments` 为什么有默认实现？** 这是 LLM 参数兼容层——当 LLM 模型升级后返回的参数格式有细微变化时（如字段名从 `filePath` 变为 `file_path`），tool 可以通过此方法做兼容转换。默认实现是 identity（不转换），保持向后兼容。
>
> **`execution_mode` 的默认值 `Parallel`：** 大多数工具（读文件、搜索、API 调用）可以安全并发执行。只有少数有副作用的工具（如写文件）需要标记 `Sequential` 以避免竞争条件。默认 `Parallel` 符合 "安全默认" 原则——如果 tool 作者不确定，标记为 `Sequential` 是显式选择。
>
> **`execute` 的生命周期：** `&'a self` 和 `&'a ToolContext` 将返回的 future 的生命周期绑定到输入。这意味着 (1) tool 在执行期间不能被释放；(2) 返回的 future 不能 spawn 到独立 task（不满足 `'static`）。后者是有意设计——tool 执行在 loop 任务内通过 `join_all` 驱动，避免线程爆炸。如果 tool 需要真正独立的任务（如长时间 shell 命令），应该内部 spawn 并 await。
>
> **`ToolContext` 的字段：**
> - `env: Arc<dyn ExecutionEnv>`——tool 通过它访问文件系统和 shell。Arc 允许 tool 在内部 clone 并用于 spawn 的 task（如果需要）。
> - `abort: CancellationToken`——用户取消时 tool 应尽快停止。CancellationToken 可以 clone 并传递给子 task。
> - `tool_use_id`——LLM 为每次 tool call 分配的唯一 ID。tool 用它关联事件和日志。
> - `turn_index`——当前轮次索引。loop 在构造 `ToolContext` 时传入，`HookedTool` 将其转发给 `BeforeToolCallCtx` / `AfterToolCallCtx`。
> - `assistant_message`——触发本次 tool call 的完整 LLM 响应。loop 在 `MessageEnd` 后构造 `ToolContext` 时传入；同一 `AssistantMessage` 可能触发多个 tool call，通过 `Arc` 共享。这是 `assistant_message` 不存储在 `HookedTool` 结构体中的原因：它不是工具的属性，而是本次调用的上下文——在 `HookedTool` 构建时（`build_loop_config()` 调用前）并不存在。
> - `update_tx`——mpsc sender。tool 执行期间通过它推送 `ToolResult` 增量。接收端（loop）将其转发为 `AgentEvent::ToolExecutionUpdate`。Channel 容量由 loop 配置——如果 tool 推送过快，send 会阻塞，自然形成背压。
>
> **`ToolResult` 的三个字段：**
> - `content: Vec<ContentBlock>`——发送给 LLM 的内容（文本、图片等）。LLM 看到的是这个。
> - `details: serde_json::Value`——不发送给 LLM 的结构化数据，用于 UI 渲染或日志分析。如 shell tool 的 `details` 可能包含 `{ "exit_code": 0, "duration_ms": 1234 }`。
> - `terminate: bool`——工具宣告任务完成的能力。当一个 batch 中**所有** tool 都返回 `terminate: true` 时，loop 提前停止（不等待 LLM 返回 `EndTurn`）。这是 "tool 自主停止" 机制——例如 "deploy" tool 部署成功后设置 `terminate: true`，agent 立即停止，不必再问 LLM "下一步做什么"。
>
> **`ToolResult.details` 持久化约定：** `details` 字段写入 session log（作为 `ToolResultMessage` 的一部分序列化为 JSON）。session 回放时不解析 `details`——它是不透明扩展数据，仅用于 UI 渲染和审计日志。compaction 不处理 `details`（不进入摘要 prompt）。

---

### 3.5 ExecutionEnv trait

> **本节设计目标：** 将执行环境（文件系统 + shell）抽象为 trait，使 Agent 可以运行在不同的环境中（本地 OS、Docker 容器、WASM 沙箱、测试 mock）。这是 pi-agent-core 后期才引入的抽象——本设计从第一天就提供。

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

> **设计理由：**
>
> **为什么每个方法都接受 `CancellationToken`？** 文件操作可能很慢（网络文件系统、大文件读取）。在用户取消后继续等待 I/O 是无意义的——CancellationToken 让实现可以提前中止。每个方法独立接受 token（而非在 struct 级别）——允许调用方对不同操作使用不同的取消策略。
>
> **为什么文件方法是细粒度的（`read_text_file` / `read_binary_file` / `read_text_lines`）而非一个通用 `read`？** (1) 文本和二进制有不同的错误语义——文本文件无效 UTF-8 是 `EnvError::InvalidUtf8`，二进制文件不会有此问题；(2) `read_text_lines` 支持 `max_lines` 限制，避免大文件撑爆内存；(3) 调用方不需要记忆 "读文本应该用哪种编码"——trait 已经做好约定。
>
> **`read_text_lines` 而非返回 `impl Iterator`：** 返回 `Vec<String>` 而非流式迭代器——简化 trait 签名（避免 GAT 或 `BoxStream`），且绝大多数场景下文件行数不会超出内存。
>
> **为什么有 `exists` 方法？** 看似可以通过 `file_info` + 检查 `NotFound` 实现，但 `exists` 的语义更清晰（返回 `bool`），且某些后端可能有更高效的实现（如 `stat` vs `access` 系统调用）。
>
> **`append_file` 而非仅 `write_file`：** JSONL session 存储的核心操作是追加行——`append_file` 允许实现使用 O_APPEND 模式，比 "读全文件 + 追加 + 写回" 高效得多。
>
> **`create_temp_dir` 为什么不接受 `CancellationToken`？** 设计选择——临时目录创建通常极快（本地 fs），不太需要取消。如果未来需要，可以加 `_cancellable` 变体。
>
> **路径操作为什么不在 trait 中？** Rust 的 `std::path::Path` 已经提供了 `join`、`parent`、`canonicalize` 等纯路径操作——它们不依赖 I/O。只有需要 I/O 的路径操作（如解析符号链接）才值得进 trait。如果 WASM 环境的路径语义不同，实现方可以在 `working_dir()` 返回的 `Path` 中编码虚拟路径前缀。
>
> **`ShellOptions` 的设计：**
> - `cwd`：覆盖工作目录。`Option` 表示 "使用 env 的默认工作目录"。
> - `env`：环境变量覆盖。用 `Vec<(&str, &str)>` 而非 `HashMap`——shell 环境变量通常很少（几个到十几个），Vec 的线性搜索对于小 N 比 HashMap 的哈希计算更快，且避免了堆分配。
> - `timeout`：`Option<Duration>`——None 表示无超时。实现方负责在超时后 kill 进程。
> - `on_stdout` / `on_stderr`：流式回调。`Box<dyn FnMut>` 而非泛型参数——保持 trait 方法的类型擦除（避免 trait 方法带泛型）。`'a` 生命周期绑定确保回调在操作期间有效。
>
> **`cleanup` 方法的必要性：** 某些 ExecutionEnv 实现持有临时资源（如创建的 temp dir）。调用方在不再需要 env 时调用 `cleanup()` 释放。这是 best-effort 方法——不保证一定成功（文件可能被锁定）。

---

**权限模型：**

> **为什么 trait 不提供细粒度权限？** ExecutionEnv 的最小接口原则——trait 定义 "能做什么"，实现方决定 "允许做什么"。`OsEnv` 实现可以限制工作目录边界、维护 shell 命令白名单。需要更细控制的场景（如 "只允许读 /tmp，不允许写"）由调用方创建一个**包装实现**——在 `read_text_file` 中检查路径前缀，不满足则返回 `EnvError::PermissionDenied`。这个包装 env 注入 `ToolContext`，框架不感知。

---

### 3.6 Hook traits

> **本节设计目标：** 提供一系列 trait（本节定义 11 个；另 `ConvertToLlmHook` 和 `CustomMessageConverter` 定义在 `llm-harness-loop` 中，因其依赖 `llm-api-adapter::Message`），允许调用方在 Agent loop 的关键决策点插入自定义逻辑。Hook 的设计原则：(1) 全部为可选——不设置 hook 时 loop 使用默认行为；(2) 全部为 `dyn Trait`——允许运行时组合；(3) 全部使用 `BoxFuture` 返回值——async 是必须支持的（hook 可能需要调外部服务）。

---

#### TransformContextHook —— 上下文转换

```rust
/// 每次 LLM 调用前对上下文做转换（compaction 通过此 hook 接入）。
pub trait TransformContextHook: Send + Sync {
    fn transform<'a>(&'a self, ctx: AgentContext)
        -> BoxFuture<'a, Result<AgentContext, AgentError>>;
}
```

> **用途：** 在 LLM 调用之前修改上下文。最典型的用例是 compaction——hook 实现检查 token 数，如果超过阈值则压缩历史消息。但也可以用于其他场景——注入系统状态、过滤敏感信息、重新排序消息。
>
> **为什么是 `AgentContext → AgentContext` 而非 `Vec<AgentMessage> → Vec<AgentMessage>`？** `AgentContext` 包含 `system_prompt` 和 `messages`——hook 可能需要修改 system prompt（如根据 token 使用情况动态调整提示）。如果只传 messages，hook 无法改变 system prompt。

---

#### PrepareNextTurnHook —— 下一轮准备

```rust
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
```

> **用途：** 每个 turn 结束后，loop 询问 hook "下一轮用什么配置？"。Harness 利用此 hook 从 session log 重建上下文——读取上次 compaction 后的有效消息、恢复 model/thinking_level/tools 配置。这实现了 "stateless loop + stateful harness" 的分离。
>
> **为什么所有 `NextTurnDirective` 字段都是 `Option`？** `None` 表示 "沿用当前值"——hook 只需要返回想改变的字段。如果每轮都必须返回完整 directive，hook 实现者被迫维护所有状态（破坏了 "Harness 管理状态" 的设计）。
>
> **`tools` 与 `active_tools` 同时非 None 的优先级：** `tools` 替换全部已注册工具（`HarnessState.tools` 被整体替换），`active_tools` 仅控制激活子集。如果两者同时非 None，先应用 `tools`（替换全集），再应用 `active_tools`（在新全集中过滤）。如果 `active_tools` 引用了不在新 `tools` 中的名称，返回 `Err(AgentError::Internal)`。
>
> **`PrepareNextTurnCtx` 的字段选择：** 提供给 hook 的信息是 "上一轮发生了什么"——turn_index（第几轮）、last_message（LLM 最后的回复）、last_tool_results（执行的工具结果）。hook 可以据此做决定（如 "连续 3 轮没有工具调用 → 切换为 faster model"）。

---

#### BeforeToolCallHook / AfterToolCallHook —— 工具拦截

```rust
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
    /// 返回 `Passthrough`（不改结果）或 `Patch`（部分覆盖）。
    /// 与 TS 版对齐：可覆盖 content / details / isError / terminate。
    fn on_complete<'a>(&'a self, ctx: AfterToolCallCtx<'a>) -> BoxFuture<'a, AfterToolCallDecision>;
}

pub enum AfterToolCallDecision {
    Passthrough,
    Patch(ToolResultPatch),
}

pub struct ToolResultPatch {
    pub content:    Option<Vec<ContentBlock>>,
    pub details:    Option<serde_json::Value>,
    pub is_error:   Option<bool>,
    pub terminate:  Option<bool>,
}
```

> **用途：** 在工具执行后插入自定义逻辑。典型场景：审计缓存（缓存成功的 tool 结果）、结果清理（脱敏 tool 输出）、副作用追踪。
>
> **`AfterToolCallDecision` 的两种结果：**
> - `Passthrough`：照常使用工具执行结果。
> - `Patch(ToolResultPatch)`：部分覆盖执行结果。`ToolResultPatch` 的所有字段均为 `Option`——`None` 表示 "保持原值"。这允许 hook 只改变它关心的字段（如只修改 `is_error` 而不碰 `content`）。与 TS 版 `afterToolCall` 的行为对齐。
>
> **为什么 `after_tool_call` context 携带 `&AssistantMessage`？** hook 可能需要知道是哪个 LLM 回复触发了这次 tool call——例如根据 `assistant_message.model` 决定是否缓存结果。

---

#### ShouldStopHook —— 停止决策

```rust
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
```

> **用途：** 当 LLM 返回 `EndTurn`/`MaxTokens` 等非 tool_use 的停止原因时，loop **不**直接停止，而是问 hook "是否应该停止？"。返回 `true` → 停止；返回 `false` → 强制再跑一轮（给 LLM 追加一条 "continue" 消息）。
>
> **为什么需要这个 hook？** LLM 有时会 "假停止"——比如在生成长内容时遇到 max_tokens 截断，调用方可能希望自动继续。hook 可以根据 `stop_reason`（是 `MaxTokens` 还是真正的 `EndTurn`）和上下文做决策。
>
> **约束：** hook **仅**在 LLM 已自然停止时被询问。它**不能**用于主动中断正在执行的 tool——中断走 `abort()`。

---

#### Provider 请求/响应 Hook

```rust
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
```

> **用途：** 在 LLM provider 调用前后插入逻辑：
> - `BeforeProviderRequestHook`：修改 stream options（动态调整 timeout、切换 transport、注入追踪 headers）
> - `AfterProviderResponseHook`：观察响应元数据（HTTP status、response headers、token usage、延迟）。用于配额追踪、成本监控、A/B 测试。
>
> **`BeforeProviderRequestHook::before_request` 为什么是 `&mut StreamOptions` 而非返回新值？** 性能——`StreamOptions` 可能包含较大的 metadata JSON，原地修改避免 clone。而且 "修改" 的语义（而非 "替换"）更贴切——多个 hook 可以链式修改同一个 options。

---

#### AuthHook —— 动态认证

```rust
/// API key / headers 动态解析（OAuth token 过期等场景）
pub trait AuthHook: Send + Sync {
    fn resolve<'a>(&'a self) -> BoxFuture<'a, Result<AuthInfo, AgentError>>;
}
pub struct AuthInfo { pub api_key: Option<String>, pub headers: Vec<(String, String)> }
```

> **用途：** 每次 LLM 调用前动态获取认证凭据。典型场景：OAuth token 每 30 分钟过期——hook 检查 token 是否仍然有效，如果过期则刷新。如果没有此 hook，token 过期会导致 LLM 调用失败，agent 中断。
>
> **为什么不是 `get_api_key` 而是返回 `AuthInfo`？** 某些 provider 通过 custom headers 认证（而非 `Authorization: Bearer`），`AuthInfo` 同时支持 api_key 和 headers。

---

#### Turn 边界 Hook

```rust
/// Harness 专属：一次完整 agent 运行（prompt 调用）开始前。
/// 对应 TS 的 BeforeAgentStartEvent。可修改 initial_messages / system_prompt。
pub struct BeforeRunCtx<'a> {
    pub prompt_text:    &'a str,
    pub initial_messages: &'a mut Vec<AgentMessage>,
    pub system_prompt:  &'a mut Option<String>,
    pub resources:      &'a AgentHarnessResources,
}
pub struct BeforeRunResult {
    pub additional_messages: Vec<AgentMessage>,
    pub system_prompt: Option<String>,
}
pub trait BeforeRunHook: Send + Sync {
    fn before_run<'a>(&'a self, ctx: BeforeRunCtx<'a>) -> BoxFuture<'a, Result<BeforeRunResult, AgentError>>;
}

/// Harness 专属：turn 边界
pub struct BeforeTurnCtx<'a> { pub turn_index: u32, pub snapshot: &'a TurnSnapshot }
pub struct AfterTurnCtx<'a>  { pub turn_index: u32, pub new_messages: &'a [AgentMessage] }
pub trait BeforeTurnHook: Send + Sync { fn before_turn<'a>(&'a self, ctx: BeforeTurnCtx<'a>) -> BoxFuture<'a, ()>; }
pub trait AfterTurnHook:  Send + Sync { fn after_turn<'a>(&'a self, ctx: AfterTurnCtx<'a>) -> BoxFuture<'a, ()>; }
```

> **用途：** Harness 层的 turn 前后通知。与 loop 层的 event 不同——loop 只发射事件，Harness 层可以**同步**地等待 hook 完成。用于：
> - `BeforeTurnHook`：turn 开始前准备（预加载上下文、通知外部系统）
> - `AfterTurnHook`：turn 结束后处理（增量索引、推送通知）

---

#### BeforeCompactHook —— Compaction 决策

```rust
/// Compaction 决策点
pub struct BeforeCompactCtx<'a> { pub estimated_tokens: usize, pub messages: &'a [AgentMessage] }
pub enum BeforeCompactDecision { Proceed, Skip, Override(CompactionResult) }
pub trait BeforeCompactHook: Send + Sync {
    fn before_compact<'a>(&'a self, ctx: BeforeCompactCtx<'a>) -> BoxFuture<'a, BeforeCompactDecision>;
}
```

> **用途：** 在 compaction 执行前允许 hook 做决策——跳过压缩（用户正在关键任务中）、使用自定义摘要（应用层已有更好的摘要）、或继续默认压缩。
>
> **`Override(CompactionResult)`：** hook 可以提供自己生成的 `CompactionResult`，完全绕过框架的摘要生成。用于应用层已有现成的对话摘要（如前置的 NL 摘要 pipeline）。

---

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

> **设计理由：**
>
> **`ThinkingLevel`：** 控制 provider 推理深度的枚举。`Off` 禁用推理（节省 token）。`XHigh` 只在特定模型上可用。各 provider 对级别的实际映射由 `llm-api-adapter` 负责转换——如 Anthropic 映射到 `budget_tokens`，OpenAI 映射到 `reasoning_effort`，DeepSeek 透传或忽略。为什么不用 `Option<NonZeroU8>`？命名级别比数字更可读——调用方不需要查表 "3 对应什么"。
>
> **`AgentContext`：** Loop 的输入——`system_prompt`（可选）和 `messages`。不包含 `tools`——工具通过 `LoopConfig.tools` 传递。这是有意分离——"说什么"（context）和 "能做什么"（tools）是独立的关注点。
>
> **`TurnSnapshot`：** 每个 turn 开始时的配置快照。`Clone` 允许廉价复制（`Arc` 的 clone 只增加引用计数）。turn 进行中对 AgentState 的修改（如 `set_model`）不会影响当前 turn——这是 "快照语义" 的核心。
>
> **`StreamOptions`：** 传递给 LLM provider 的传输层配置。`metadata` 和 `cache_config` 是 `serde_json::Value`——厂商特定的字段由调用方自由填充，框架不做假设。`headers` 用 `Vec<(String, String)>` 而非 `HashMap`——HTTP headers 通常很少（几个），且需要保持插入顺序。
