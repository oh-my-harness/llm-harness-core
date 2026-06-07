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
