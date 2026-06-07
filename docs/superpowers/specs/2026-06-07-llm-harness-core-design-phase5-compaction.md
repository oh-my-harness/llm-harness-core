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
