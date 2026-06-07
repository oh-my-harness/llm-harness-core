### 5.4 Compaction（基于 session entries）

> **Compaction 是什么？** 当对话历史过长（超过 LLM 上下文窗口的某个阈值），需要将旧消息 "压缩" 为一段摘要。后续每轮 LLM 调用用摘要替代原始历史——节省 token 的同时保留关键上下文。这不是简单的截断——截断丢失的信息可能恰好是 LLM 需要的关键决策上下文。

**关键修正：** Compaction 操作的是 **session path entries**，而非纯 message 数组。这样才能：
1. 在路径中定位上次 compaction 的 `first_kept_entry`
2. Cut point 落在 entry 边界，不在 toolResult 中间截断
3. 输出新的 `first_kept_entry` 作为下一次 compaction 边界

> **为什么必须基于 session entries？** 纯 message 数组方案的根本缺陷：(1) 不知道上次 compaction 在哪里——无法实现迭代摘要（在已有摘要基础上更新而非从头摘要）；(2) 不知道 entry 类型——可能在 toolResult 的中间截断，导致 LLM 看到不完整的 tool 调用链；(3) 无法输出 `first_kept_entry`——压缩后不知道 "从哪里开始历史仍然有效"，下一次 compaction 无法确定边界。

---

#### 配置与数据结构

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
```

> **设计理由：**
>
> **`enabled` 开关：** 允许调用方完全禁用 compaction（如调试 session 时不想丢失任何消息）。默认 `true`。
>
> **触发条件 `context_window - reserve_tokens`：** `reserve_tokens` 是为 LLM 响应预留的空间。如果 context_window = 200k，reserve_tokens = 16k，那么当 token 数 > 184k 时触发 compaction。`reserve_tokens` 需要覆盖：(1) 摘要 prompt 的 token 数，(2) 新用户消息的 token 数，(3) LLM 回复的 token 数（至少 `max_tokens` 的量）。默认值 16384 是实践中的安全值。
>
> **`keep_recent_tokens`：** "保留最近的 N tokens 不压缩"——确保 LLM 总能看到最近的消息。这是从用户体验出发的——最近的对话是 LLM 继续工作的最重要上下文。默认 20000 tokens（约 40 页文本）。
>
> **`summary_model` + `summary_model_info`：** 为什么分开？`summary_model` 是摘要模型的名称（如 `"claude-haiku-4-5"`），`summary_model_info` 包含该模型的 context_window、max_tokens 等元数据。compaction 需要 `summary_model_info.context_window` 来确保摘要 prompt 不会超过摘要模型的上下文窗口。
>
> **为什么 compaction 可以用更便宜的模型？** 摘要任务是 "读出关键信息并结构化输出"——不需要复杂推理，便宜的模型（如 Haiku）通常足够。这是成本优化——主对话用 Sonnet/Opus（贵但强），摘要用 Haiku（便宜但够用）。

---

```rust
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
```

> **两段式设计（prepare + compact）的理由：**
> - `prepare_compaction()`：纯函数——基于 session entries 做决策，不调 LLM。返回 `None`（不需压缩）或 `CompactionPreparation`。
> - `compact()`：调用 LLM 生成摘要，可能耗时数秒。返回 `CompactionResult`。
>
> **分离的好处：** (1) 决策逻辑可独立测试（给定 entries，验证 cut point 是否正确）；(2) 调用方可以在 prepare 和 compact 之间插入逻辑（如通过 `BeforeCompactHook` 修改或跳过）；(3) 如果决策是不需压缩，完全避免 LLM 调用。
>
> **`CompactionPreparation` 的字段：**
> - `path_entries`：完整的 session path（从 root 到 active leaf），用于 LLM 摘要的输入。
> - `first_kept_entry`：上一次 compaction 的 first_kept_entry（如果存在）。作为本次 compaction 的 "起始边界"——只压缩它之后的 entry。
> - `cut_point`：截断点——此 entry 之前的全部压缩，此 entry（含）之后全部保留。
> - `previous_summary`：上一次 compaction 的摘要文本。用于迭代摘要——新摘要是 "在已有摘要基础上，加入新消息" 的更新，而非从头重写。
> - `estimated_tokens`：压缩前的 token 估算数。用于 UI 显示和日志。
> - `split_turn_prefix`：当 cut_point 落在 turn 中间时（不是 user message 边界），需要独立摘要 turn 的前半部分。
> - `file_operations`：跨多次 compaction 累积的文件读写列表。LLM 在摘要中被告知 "此期间操作了这些文件"。

---

```rust
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

> **`prepare_compaction` 的内部逻辑（概述）：**
> 1. 从后向前遍历 path entries，找到最近一次 compaction entry（`last_compaction` 参数）。
> 2. 如果存在，从 `last_compaction.first_kept_entry` 开始计算边界（只压缩 compaction 之后的消息）。
> 3. 估算当前总 token 数。
> 4. 判断是否超过阈值：`estimated_tokens > model_info.context_window - settings.reserve_tokens`。
> 5. 如果未超过，返回 `None`（不需压缩）。
> 6. 如果超过，从后向前扫描 entries，累计 token 直到达到 `keep_recent_tokens`——找到 cut point。
> 7. 验证 cut point 合法性。**Turn 边界：** 一条 UserMessage + 一条 AssistantMessage + 其所有 ToolResultMessage。**合法 cut point：** UserMessage（或其等价物，如 BranchSummary）之前；或 CompactionEntry 之后。**不合法：** ToolResultMessage 之前（孤立的 tool result）。若不合法，向前（向更旧方向）移动到最近的合法边界。
> 8. 如果调整后的 cut point 落在 turn 中间（分割了完整的 assistant + tool_results），标记 `split_turn_prefix`——被分割的 turn 前半部分独立生成摘要。`split_turn` 场景的两次摘要调用**并行执行**（`futures::join!` 或 `tokio::join!`），结果合并为 `"{history_summary}\n\n---\n\n**Turn Context:**\n\n{turn_prefix_summary}"`。
> 9. 提取文件操作列表。
> 10. 返回 `CompactionPreparation`。
>
> **`compact` 的内部逻辑（概述）：**
> 1. 将 `preparation.path_entries` 中被压缩的部分序列化为文本。
> 2. 如果存在 `previous_summary`，使用更新摘要 prompt（"在已有摘要基础上加入新信息"）；否则使用初始摘要 prompt。
> 3. 调用 `summary_model`（通过 `client`）生成摘要。
> 4. 如果有 `split_turn_prefix`，独立为 turn 前缀生成摘要。
> 5. 合并摘要文本 + 文件操作列表。
> 6. 构造 `CompactionResult`。
>
> **`CompactionResult` 的字段：**
> - `summary_message`：CompactionSummaryMessage——将被写入 session log 并注入到后续上下文中。
> - `first_kept_entry`：cut_point 的 entry id——下一次 compaction 的起始边界。写入 CompactionEntry。
> - `tokens_before` / `tokens_after`：压缩前后的 token 估算数。UI 可显示 "压缩节省了 X tokens"。
> - `file_operations`：此期间的文件操作列表——附加到摘要末尾，LLM 知道 "这些文件被改动过"。
>
> **`FileOperation`：** `path` 是文件路径，`kind` 是 Read 或 Modify，`at_entry` 是此操作发生的 entry id。用于诊断——"为什么 compaction 后 LLM 不知道我改了 X？→ 检查 file_operations 列表"。

---

**LLM 调用归属：** 独立于 Agent 主循环，由 Harness 用 `summary_model` 直接调 `LlmClient`。Agent 不感知。

> **为什么 compaction 的 LLM 调用不经过 agent_loop？** (1) compaction 是同步阻塞操作——它必须完成才能继续对话；(2) 它使用不同的模型（summary_model vs 主模型）；(3) 它的 prompt 和 system prompt 是框架内置的（`SUMMARIZATION_SYSTEM_PROMPT`），与 Agent 的 system prompt 无关；(4) 它不应该触发 Agent 事件（用户不需要看到 "摘要生成中" 的流式输出）。

**失败回滚语义：** `compact()` **不**写入 session——它只返回 `CompactionResult`。调用方（Harness）拿到 `Ok(result)` 后才调用 `session.append(SessionEntryPayload::Compaction(...))`。如果 `compact()` 返回 `Err`，session 完全未被修改——无需回滚。`CompactionPreparation` 只是临时分析结果，不影响持久状态。

**摘要 prompt 模板（参考 TS 版本，实现阶段最终调优）：**

```
SYSTEM: You are a context summarization assistant. Your task is to read a conversation
        between a user and an AI coding assistant, then produce a structured summary...

USER:   The messages above are a conversation to summarize. Create a structured context
        checkpoint summary that another LLM will use to continue the work.

        ## Goal
        [What is the user trying to accomplish?]

        ## Progress
        ### Done
        - [x] [Completed tasks]
        ### In Progress
        - [ ] [Current work]

        ## Key Decisions
        - **[Decision]**: [Brief rationale]

        ## Next Steps
        1. [Ordered list]

        ## Critical Context
        - [File paths, function names, error messages to preserve]

迭代更新模式：若存在 previous_summary，prompt 改为 UPDATE 变体——"The messages above are NEW
conversation messages to incorporate into the existing summary provided in <previous-summary> tags."
要求保留已有信息、添加新进展、更新 In Progress → Done。
```

**阶段交互：** v1 `harness.compact()` 仅在 Harness Idle 时可调用，期间转为 `Compacting`。v1.x 可考虑后台压缩。

> **为什么 v1 不实现后台压缩？** 后台压缩（agent 运行期间异步压缩）引入了复杂性：(1) session entry 可能在被压缩的同时被新消息追加——需要 MVCC 或乐观锁；(2) 如果压缩完成时 agent 已经在使用新上下文——需要决定是否打断；(3) 压缩期间的错误恢复更复杂。v1 选择简单——同步压缩在 Idle 时执行，保证了数据一致性但可能阻塞用户交互。
