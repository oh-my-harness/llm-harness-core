use crate::EntryId;

/// Compaction 结果——由 Phase 5 完整定义；此处为 Phase 1 的 stub。
pub struct CompactionResult {
    /// 摘要文本。
    pub summary: String,
    /// 压缩后保留的第一条 entry 的 ID（`None` = 全量压缩）。
    pub first_kept_entry_id: Option<EntryId>,
}
