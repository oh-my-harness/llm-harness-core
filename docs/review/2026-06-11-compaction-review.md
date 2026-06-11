# Code Review 报告：compaction.rs

## 审查范围
- **Spec**: `docs/superpowers/specs/2026-06-07-llm-harness-core-design-phase5-compaction.md`
- **Tasks**: N/A
- **当前 Task**: N/A
- **审查文件**: `crates/llm-harness/src/compaction.rs`

## 总体结论: NEEDS_CHANGES

---

## 正式问题

### P0（必须修复）

#### P0-1: `estimate_tokens_for_message` 使用请求级 `input_tokens` 导致 token 估算 O(K²) 膨胀
- **维度**: 健壮性 / 契约
- **位置**: `compaction.rs:126–131`
- **问题**: 对有 `usage` 的 `AssistantMessage` 返回 `usage.input_tokens + usage.output_tokens`。`input_tokens` 是该轮 LLM 调用的全量输入 token 数（含历史），不是该消息的自身大小。K 轮对话后 `estimated_tokens` ≈ O(K²)，在短对话中就触发 compaction，且 cut point 被错误地推向最近内容。
- **证据**: `messages.rs:9` 注释"Prompt token 数"；10 轮对话后 `estimated_tokens` 约为实际值的 10 倍。
- **建议**: 改为 `return usage.output_tokens as usize;`（仅计该消息自身产生的 token），或完全使用 `estimate_tokens_for_content_blocks` 代替 usage 快速路径。

#### P0-2: `let _ = (name, input)` 在 `content_blocks_to_text` 中触发 clippy lint，阻塞 CI
- **维度**: 工程规范
- **位置**: `compaction.rs:401–404`
- **问题**: `let _ = (name, input);` 是 clippy `-D warnings` 下的错误（unused variable binding）。CLAUDE.md 要求提交前必须通过 `cargo clippy --all-targets --all-features`，此处会导致 CI 硬性失败。注释写"Include tool call summary inline"但实现返回 `None`，意图与代码相悖。
- **证据**: 项目规范 CLAUDE.md §提交前必做清理。
- **建议**: 改为 `ContentBlock::ToolUse { name, input, .. } => Some(format!("[Tool: {}] {}", name, input))`（同时修复 ToolUse 内容缺失问题），或至少改为 `ContentBlock::ToolUse { .. } => None`（消除 lint）。

---

### P1（应该修复）

#### P1-1: 第二次及后续 compaction 向摘要 LLM 提交已被前次覆盖的旧历史
- **维度**: 契约 / 正确性
- **位置**: `compaction.rs:269`
- **问题**: `entries_to_compress = &preparation.path_entries[..cut_idx]` 从 path index 0 开始，包含上次 compaction 已覆盖的旧消息（`path_entries[0..start_idx]`）。这些旧消息出现在 LLM prompt 的 `<conversation>` 块中，与 `<previous-summary>` 中的旧摘要重复，LLM 被告知"incorporate these NEW messages"但其中许多是已摘要内容。`preparation.first_kept_entry` 字段携带了正确边界但在 `compact()` 中从未被读取。
- **证据**: `format_entries_as_text` 的 `_ => {}` 分支不序列化 `Compaction` payload（`compaction.rs:362`）；`compact()` 函数体中 `preparation.first_kept_entry` 无任何引用。
- **建议**: 在 `compact()` 中增加下界计算：
  ```rust
  let first_kept_idx = preparation.path_entries.iter()
      .position(|e| e.id == preparation.first_kept_entry)
      .unwrap_or(0);
  let entries_to_compress = &preparation.path_entries[first_kept_idx..cut_idx];
  ```

#### P1-2: `ToolUse` 内容块被静默丢弃，摘要缺失工具调用上下文
- **维度**: 健壮性 / 质量
- **位置**: `compaction.rs:401–404`
- **问题**: `content_blocks_to_text` 对 `ContentBlock::ToolUse` 返回 `None`，工具调用名称和参数完全不出现在传给摘要 LLM 的对话文本中。含大量工具调用的对话被压缩后，LLM 只能看到工具结果而看不到触发它的调用，摘要中的"Key Decisions"和"Critical Context"会缺少工具操作记录。注意 P0-2 和 P1-2 可以一步修复。
- **建议**: 改为 `Some(format!("[Tool: {}({})]", name, input))`（可简化 input 为紧凑表示）。

#### P1-3: `SUMMARY_MAX_TOKENS` 硬编码 4096，与 `summary_model_info.max_tokens` 静默不一致
- **维度**: 需求符合度
- **位置**: `compaction.rs:15, 292`
- **问题**: `compact()` 构造 `ChatRequest` 时使用 `SUMMARY_MAX_TOKENS = 4096`，而 `CompactionSettings.summary_model_info.max_tokens`（调用方可配置）从未被引用。当调用方将 `max_tokens` 配置为小于 4096 的值（如 2048）时，API 请求超出模型限制导致 400 错误。
- **建议**: 改为 `settings.summary_model_info.max_tokens as u32`，删除 `SUMMARY_MAX_TOKENS` 常量。

#### P1-4: `compact()` 未验证 prompt 长度是否超过 `summary_model_info.context_window`
- **维度**: 需求符合度
- **位置**: `compaction.rs:292–297`
- **问题**: spec 明确："`summary_model_info` 必需用于 token 估算，确保摘要 prompt 不超过摘要模型的上下文窗口"。`compact()` 构造完 `user_content` 后直接发出 LLM 请求，不检查长度。历史很长或摘要模型窗口较小时，调用失败的 LLM API 错误远不如提前检测清晰。
- **证据**: `settings.summary_model_info` 在 `compact()` 函数体中零引用。
- **建议**: 在构造 `req` 前估算 `user_content` token 数，若超过 `settings.summary_model_info.context_window - settings.summary_model_info.max_tokens` 则截断或返回 `Err(CompactionError::SummaryFailed(...))`。

#### P1-5: `split_turn_prefix` 始终为 None，turn 内部 cut 场景静默降级无告警
- **维度**: 需求符合度
- **位置**: `compaction.rs:224`
- **问题**: spec 步骤 8 要求当 cut point 落在 turn 内部时独立生成 turn 前缀摘要并并行合并。v1 置 None 可接受，但含 tool use 的长对话发生此场景时无任何降级告警，调用方无法感知摘要质量损失。
- **建议**: 明确标注为 v1 限制；发生此场景时（cut point 前紧跟 `AssistantMessage`）至少记录诊断信息。

#### P1-6: `file_operations` 始终为空，session 数据残缺
- **维度**: 需求符合度
- **位置**: `compaction.rs:225`
- **问题**: spec 步骤 9 要求从 entries 中提取文件操作列表附加到摘要末尾。`prepare_compaction` 返回 `file_operations: vec![]`，`compact()` 中的 `## Files Touched` 分支（`compaction.rs:312-323`）永远不执行。
- **建议**: 若确认为 v1 有意简化，在代码注释中明确标注；否则实现从 `ToolResult` 内容中提取文件路径。

#### P1-7: `pub(crate)` 函数缺少 `///` doc comment
- **维度**: 工程规范
- **位置**: `compaction.rs:140`（`estimate_tokens_for_entry`），`compaction.rs:350`（`format_entries_as_text`）
- **问题**: 项目规范要求所有 `pub`/`pub(crate)` 函数有 `///` doc comment；这两个函数被 `harness.rs` 调用，属于 crate 内公共接口，但无文档。

---

### P2（建议改进）

- `estimate_tokens_for_entry` 无直接单元测试，`ModelChange`/`Label` 等 payload 返回 0 的行为未经验证
- `is_valid_cut_start` 未处理 `SessionEntryPayload::Compaction` 变体（spec 说"CompactionEntry 之后"也是合法 cut start；需确认与现有实现是否等价）
- `SUMMARIZATION_SYSTEM_PROMPT`/`SUMMARY_REQUEST` 私有常量无 `///` doc，与同文件已有 doc 的常量风格不一致
- 性能：`path.to_vec()`（`compaction.rs:219`）在触发时对整个 path 做深拷贝；可考虑将 `path_entries` 改为 `Arc<Vec<...>>` 或仅存索引

---

## Follow-up Notes

- `compact()` 中 `unwrap_or(preparation.path_entries.len())`（`compaction.rs:267`）在 v1 单线程 Idle 下不可触发，但语义上应为 `return Err(...)` 而非静默回退，建议加注释或改写
- `BeforeCompactDecision::Override` 路径下，hook 提供的 `first_kept_entry` 写入 session 后若不在下次 path 中，`start_idx` 回退为 0 导致冗余重压缩——建议在文档中注明调用方契约
- P0-2 和 P1-2 可以一步修复：为 `ToolUse` 生成简要文本摘要，同时消除 clippy lint
