### 5.3 Session（多分支树）

**设计哲学：** 会话是类型化追加日志，每条 entry 含 `parent_id` 形成**真正的树**。多个 leaf 表示并存的分支；用户可在任意历史 entry 上 fork 出新分支；`navigate_to(target)` 切换写入位置。

> **为什么是树而非线性日志？** 线性日志只能表达 "一条时间线"。但实际使用中，用户经常需要从历史点重新尝试（"回到上一步，换个思路"）。树结构允许从任意 entry fork 出新分支，每条分支独立演进。分支之间共享前缀路径（root → fork_point），节省存储。
>
> **"类型化追加日志" 的含义：** 每条 entry 不仅是字节 blob——它有类型（Message、Compaction、BranchPoint...），类型决定了它在上下文重建中的处理方式。这是 "结构化日志" 而非 "纯文本日志"。

**核心概念：**
- **Entry tree**：所有 entry 按 `parent_id` 链接成树（root 的 parent = None）
- **Leaf**：任何一个没有子 entry 的节点；每条分支对应一个 leaf
- **Active cursor** (`active_cursor`)：下一次 append 时新 entry 的 `parent_id` 指向。命名**不**用 "active_leaf"——fork 操作会把 cursor 临时指向树的内部节点（非 leaf），下一条 append 才创造出新 leaf。
- **Branch**：从 root 到任一 leaf 的路径
- **Fork**：把 cursor 指向某历史 entry 后追加，新 entry 自然成为新分支的起点
- **Cross-session fork**：把整条路径复制到新 session 作为独立时间线

> **Active cursor vs Active leaf 的命名选择：** 在 fork 操作中，cursor 被设置为 `from_entry`（一个有子节点的内部节点）。此时 cursor 指向的不是 leaf——称它为 "active leaf" 是误导。`active_cursor` 准确反映了 "下一次写入的 parent" 语义。当新 entry 被 append 后，cursor 自动更新为新 entry（此时它确实是 leaf）。

---

#### Session Entry 数据结构

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

> **设计理由：**
>
> **`SessionEntry` 的字段分离：** `id`、`parent_id`、`timestamp` 是所有 entry 的公共元数据——提取到外层 `SessionEntry` struct 中，`payload` 是类型特定的数据。这样 (1) 树查询（`children`、`path_to_root`）只需要读外层字段，不需要解析 payload；(2) payload 的序列化格式可以独立演化。
>
> **`SessionEntryPayload` 的变体设计：**
> - `Message(AgentMessage)`：最常用的 entry 类型——占 session 内容的 90%+。直接嵌入 `AgentMessage` 无需拆包。
> - `ModelChange`、`ThinkingLevelChange`、`ActiveToolsChange`：配置变更记录。在 session 回放时，按顺序 apply 这些变更即可恢复任意时刻的运行时配置。`ModelChange` 携带 `provider` 和 `model_id`（不只是 model name）——因为同一个 model name 可能对应不同的 provider（如 `gpt-4` 在 OpenAI 和 Azure 上）。
> - `Compaction(CompactionEntry)`：压缩记录。`first_kept_entry` 是关键字段——指向 "此压缩后仍然有效的第一个 entry"。下一次 compaction 从它开始计算边界，形成链式引用。
> - `Label { name }`：给当前位置打标签（如 "working solution"、"bug introduced here"），用于导航。
> - `SessionInfo { name }`：会话重命名。独立 entry 类型——会话可以有多个命名事件（用户多次改名），最新一条生效。
> - `Custom { type, data }`：应用层扩展入口。任何框架不理解的 entry 类型都可以通过 Custom 表达。
> - `BranchPoint`：**语义标注，非强制性**。任何 entry 都可以成为 fork 起点（因为树结构允许任意 parent_id），BranchPoint 只是告诉 UI "这里有分支，请高亮显示"。
> - `BranchSwitch`：记录 cursor 切换事件。在 session 回放时，遇到 `BranchSwitch` 意味着 "从这里开始，走另一条分支"。这使得 session log 可以完整重放包括分支切换在内的所有操作。
> - `BranchSummary(BranchSummaryEntry)`：AI 为某条分支生成的摘要，存储在分支的 leaf 附近。
>
> **`CompactionEntry` 的字段：**
> - `summary_message`：CompactionSummaryMessage——压缩后插入到上下文中的摘要消息。
> - `first_kept_entry`：压缩后第一个仍然有效的 entry 的 ID。这是一个**前向引用**（指向压缩之前的某个 entry）。在构建上下文时，遇到 Compaction entry → 找到 `first_kept_entry` → 跳过 `first_kept_entry` 之前的所有消息 → 从 `first_kept_entry` 开始继续。
> - `tokens_before`：压缩前估算的 token 数。用于 UI 显示 "压缩了 X tokens"。
> - `from_hook`：是否由 `BeforeCompactHook` 提供（而非框架生成）。用于审计——知道摘要的来源。
> - `details`：不透明扩展数据（如 TS 的 `CompactionDetails { readFiles, modifiedFiles }`）。
>
> **`BranchSummaryEntry` 的字段：**
> - `leaf_id`：被摘要的分支的 leaf。用于关联——"这个摘要是关于哪条分支的？"
> - `from_entry`：摘要覆盖的起始 entry（含）。从 `from_entry` 到 `leaf_id` 的路径就是摘要的范围。
> - `summary`：AI 生成的摘要文本。
> - `token_count`：被摘要的路径的 token 估计数。UI 可显示 "此分支包含约 X tokens 的对话"。

---

#### 存储层

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

    fn find_entries_by_type<'a>(&'a self, kind: SessionEntryKind)
        -> BoxFuture<'a, Result<Vec<EntryId>, SessionError>>;
}

/// SessionEntry 类型枚举——用于 `find_entries_by_type` 的类型安全参数。
/// 实现 Display 以便日志输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEntryKind {
    Message, ModelChange, ThinkingLevelChange, ActiveToolsChange,
    Compaction, Label, SessionInfo, Custom,
    BranchPoint, BranchSwitch, BranchSummary,
}
```

> **`SessionStorage` trait 设计理由：**
>
> **为什么 `create_entry_id` 是同步方法？** UUIDv7 的生成不涉及 I/O——纯 CPU 操作（获取当前毫秒时间戳 + 随机数）。同步方法避免不必要的 async 开销。
>
> **`path_to_root` vs `children` vs `all_leaves`：** 三个不同方向的树查询——
> - `path_to_root`：从某个节点向上走到 root——用于 `build_context`（获取活跃分支的完整历史）。
> - `children`：从某个节点向下看直接子节点——用于树形 UI 展示。
> - `all_leaves`：找到所有叶节点——用于 `list_branches`（展示所有分支）。
>
> **`common_ancestor`：** 找到两个 entry 的最近公共祖先。分支 diff 的基础——当用户从 leaf A 切换到 leaf B 时，`common_ancestor(A, B)` 确定分歧点。
>
> **`find_entries_by_type`：** 按 entry 类型查找。最常用场景——找到 session 中所有 `Compaction` entry（compaction 边界追踪），找到所有 `Label` entry（构建标签索引）。
>
> **`label_at` 而非 `get_label(id)`：** label entry 通过 `targetId` 引用另一个 entry。`label_at(id)` 找到 "引用 id 的 label"（如果有）。这是间接查询——label entry 的 target_id 字段指向被标记的 entry。
>
> **为什么没有 `update_entry`？** 追加日志不可变——entry 一旦写入就不能修改。这简化了并发模型（无写-写冲突）和缓存（entry 不会失效）。如果需要 "修改"，追加一条新的 entry 并让 `build_context` 的逻辑覆盖旧值（如 ModelChange 的新值覆盖旧值）。
>
> **所有方法返回 `BoxFuture` 而非 `async fn`：** trait 中不能使用 `async fn`（需要固定的 Future 类型）。`BoxFuture` 是类型擦除的 async future——额外的堆分配（一次），但保持了 trait 的 object safety。

---

**仓库抽象：**

```rust
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
```

> **为什么需要 `SessionRepo` 而不仅是 `SessionStorage`？** `SessionStorage` 管理**单个 session** 的内容。`SessionRepo` 管理**多个 session** 的生命周期——创建、打开、列出、删除、fork。这相当于文件系统 vs 文件——你不需要为每个文件实现一个文件系统。
>
> **`fork` 的语义：** 从 source session 复制路径到新 session。新 session 独立于 source——后续写入不会影响 source。v1 强制 `copy_entries: true`——实体复制所有 entry，新 session 有独立的 entry ID。`copy_entries: false`（引用模式）涉及跨 session 的 entry ID 引用——更复杂，推迟到 v1.x。
>
> **`fork` 返回 `Arc<dyn SessionStorage>` 而非 `Session`：** Repo 返回底层 storage，调用方用 `Session::new(storage)` 包装。分离是为了让调用方可以在 storage 和 Session wrapper 之间插入自定义逻辑。

---

#### 高层 Session 接口

```rust
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

> **高层 Session 方法的设计理由：**
>
> **`read_active_path` vs `build_context`——双层读取：**
> - `read_active_path`：返回原始 entry 列表（含所有 entry 类型：ModelChange、Compaction、BranchPoint...）。用于需要完整 session 数据的场景（compaction 准备、分支 diff、session 导出）。
> - `build_context`：返回 "当前有效上下文"——跳过被 Compaction 覆盖的历史，应用摘要消息，提取最后已知的 model/thinking_level/tools。这是 AgentHarness 每轮重建上下文时调用的方法。
>
> **`build_context()` 算法（伪代码）：**
> ```rust
> async fn build_context(&self) -> Result<BuiltContext> {
>     let entries = self.read_active_path().await?;  // root → leaf
>     let mut messages = Vec::new();
>     let mut last_model = None;
>     let mut last_thinking = None;
>     let mut last_tools = None;
>     // 找到最近一次 CompactionEntry（若有）
>     let last_compaction = entries.iter().rev()
>         .find_map(|e| if let SessionEntryPayload::Compaction(c) = &e.payload { Some(c) } else { None });
>     // 确定起始位置：若有 compaction，从 first_kept_entry 之后开始
>     let start_from = last_compaction.map(|c| c.first_kept_entry);
>     let mut skip_until_compaction = start_from.is_some();
>     for entry in &entries {
>         // 跳过被 compaction 覆盖的旧 entry
>         if skip_until_compaction {
>             if Some(entry.id) == start_from { skip_until_compaction = false; }
>             else { continue; }
>         }
>         match &entry.payload {
>             SessionEntryPayload::Message(msg) => messages.push(msg.clone()),
>             SessionEntryPayload::Compaction(c) => {
>                 // 将 CompactionSummaryMessage 插入到消息列表最前面
>                 messages.insert(0, c.summary_message.clone());
>             }
>             SessionEntryPayload::ModelChange { to, .. } => last_model = Some(to.clone()),
>             SessionEntryPayload::ThinkingLevelChange { to } => last_thinking = Some(*to),
>             SessionEntryPayload::ActiveToolsChange { active } => last_tools = Some(active.clone()),
>             // BranchSwitch / BranchPoint / Label / Custom 等——跳过，不影响消息上下文
>             _ => {}
>         }
>     }
>     Ok(BuiltContext { messages, last_model, last_thinking, last_active_tools: last_tools })
> }
> ```
>
> **`append_message` / `append`——内部自动填充：** 调用方只提供 payload，Session 自动填充 `id`（调用 `storage.create_entry_id()`）、`parent_id`（读取 `storage.active_cursor()`）、`timestamp`（`Utc::now()`），并将 `active_cursor` 更新为新 entry 的 id。这确保了 "追加→cursor 自动前进" 的语义——调用方不需要手动管理 cursor。
>
> **`navigate_to`——切换分支：** 将 `active_cursor` 设置为 `target`（可以是任意 entry），写入 `BranchSwitch` entry 作为审计记录。此后 `append` 会在 `target` 下创建子 entry——如果 `target` 已有其他子节点，新 entry 成为兄弟节点（新分支）。
>
> **`fork_branch`——创建新分支：** 这是 "在现有对话中创建分支" 的操作。与跨 session fork 不同——这不会创建新 session。步骤：(1) cursor 切到 `from_entry`；(2) 写入 `BranchPoint` entry（UI 标注用）；(3) 后续 `append` 自然形成新分支。`label` 参数用于 UI 显示分支名称。
>
> **`delete_branch`——删除算法：** 从指定 leaf 沿 `parent_id` 向上删除 entry，直到遇到一个 "有多个子节点" 的 entry（即此 entry 还被其他分支共享）。停止于共享节点之前——保证不破坏其他分支。
>
> **`BuiltContext` 的字段：** `messages` 是当前有效消息列表，`last_model`/`last_thinking_level`/`last_active_tools` 是路径中最后一次 ModelChange/ThinkingLevelChange/ActiveToolsChange 的值。AgentHarness 用这些字段恢复运行时配置。
>
> **`BranchInfo`：** `list_branches()` 的返回值——每个 leaf 一条。`message_count` 是该分支的 entry 总数（用于 UI 显示分支大小），`last_activity` 是最新 entry 的时间戳，`summary` 来自最近的 `BranchSummary` entry（如果存在）。
>
> **两个 Repo 实现：** `JsonlSessionRepo` 用于生产——session 持久化为 `.jsonl` 文件。`InMemorySessionRepo` 用于测试和不需要持久化的场景（如临时会话）。

---

**并发安全：** `Session` 的方法签名是 `&self`——允许多 caller 共享 `Arc<Session>`。串行化在 `SessionStorage` 实现内：
- `JsonlSessionStorage` 内部持有 `tokio::sync::Mutex<Internal>`，所有 append/set_active_cursor/path query 串行化
- 高层组合操作（如 `fork_branch` = set_active_cursor + append BranchPoint）在 Session 内通过持有同一锁保证两步原子
- 调用方仍需注意逻辑层并发——同时 `navigate_to(A)` 与 `navigate_to(B)` 不会损坏存储，但最终落到哪个是非确定性的

> **为什么 `Session` 用 `&self` 而非 `&mut self`？** `Arc<Session>` 可以被多个 task 共享——UI task 读取分支列表，agent task 同时追加消息。如果用 `&mut self`，调用方被迫在外层加 `Mutex<Session>`，违背了 "Session 内部已有并发控制" 的设计。`&self` + 内部 `Mutex` 是 Rust 中共享状态的惯用模式。
>
> **逻辑层并发的不确定性：** 两个 task 同时调用 `navigate_to(A)` 和 `navigate_to(B)`——两个操作都会被串行化执行（storage 内部锁），但执行顺序取决于调度。最终 cursor 指向 A 还是 B 是不确定的。这不是 bug——调用方应避免这种竞争。如果需要确定性，调用方在外层排队。

---

**JSONL 存储策略 + 缓存：**
- 单 session 单文件，所有 entry 按时间 append；树结构由 `parent_id` 重建
- `active_cursor` + metadata 持久化到独立的 `.meta.json`
- `JsonlSessionStorage` 内部维护 in-memory 树缓存（`HashMap<EntryId, SessionEntry>` + `HashMap<EntryId, Vec<EntryId>>` 子节点表）：
  - 首次访问时 mmap/读全量文件构建
  - 每次 `append_entry` 增量更新缓存（O(1) 插入）
  - `path_to_root` / `children` / `all_leaves` 直接走内存，O(path length) 或 O(1)
- 大文件（>10k entries 或 >100MB）：不在首次访问时构建完整内存树，改为按需从文件扫描——`path_to_root` 反向扫描，`children` 正向扫描 + 过滤。避免大 session 内存峰值，但每次查询有 I/O 开销。正常 session（<10k entries）始终全量内存缓存

> **JSONL 格式的理由：** 每行一个 JSON 对象——天然支持 append（不需要重写整个文件），人类可读（调试友好），逐行解析内存友好（不需要一次加载整个文件）。相比 SQLite——更简单的依赖（不需要 `libsqlite3`），更容易跨平台（WASM），且 session 的访问模式是 "顺序重放" 而非 "随机查询"。
>
> **缓存策略：** 首次 `path_to_root` 调用时从文件构建完整的内存树（entry 表 + 子节点表）。后续所有树查询走内存。`append_entry` 同时写文件和更新内存缓存——保持两者同步。代价是内存占用——假设每条 entry 平均 2KB，10 万条 entry 约 200MB。对于大多数 session（<1 万条），缓存占用约 20MB——完全可以接受。
>
> **`.meta.json` 分离：** 元数据（session id、name、active_cursor）频繁更新（每次 append 都改 active_cursor）。分离为小文件避免了每次 append 都重写整个 session 文件。

---

**分支摘要生成：** `generate_branch_summary(client, session, leaf)` 在 Harness 层提供，调用 `summary_model` 生成 BranchSummary entry。

> **为什么在 Harness 层而非 Session 层？** 分支摘要需要调用 LLM——这涉及 `LlmClient`、model 选择、认证管理。Session 是 "纯存储" 层——它不应该知道 LLM 的存在。Harness 作为编排层协调两者。

**Compaction 与分支的交互：** 每个分支独立追踪 compaction 历史——`first_kept_entry` 沿 parent_id 链向上追溯，找到本分支最近一次 Compaction entry 作为基准。fork 出的新分支继承父分支的 compaction 历史。

> **"继承" 的含义：** 当从 fork_point 创建新分支时，新分支的初始 compaction 状态与源分支在 fork_point 处的状态相同。之后新分支上的 compaction 独立于源分支——在分支 A 上压缩不会影响分支 B 的上下文。
