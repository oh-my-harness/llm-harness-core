# llm-harness-core 设计文档审查报告

**日期：** 2026-06-07
**审查方法：** 4 个独立 agent 分别审查完整性、设计忠实度、内部一致性、实现指导充分性；pi-main TypeScript 原版作为参照基准。

---

## 总体评估

文档整体结构清晰，模块分层合理，设计理由注释覆盖面好。主要问题集中在三个方面：

1. **内部一致性**：多处跨文件的类型定义直接矛盾，部分伪代码与正文设计原则自相矛盾，会直接导致实现者产生不同理解。
2. **实现指导充分性**：Compaction 核心算法（cut point 验证、prompt 模板、失败回滚）、`build_context()` 完整逻辑、`run_loop` 关键写入路径细节不足以指导实现。
3. **设计忠实度**：`AfterToolCallHook` 丢失了 TS 版的结果修补能力，属于功能性退化而非 Rust 惯用法适配。

完整性方面问题均为警告级别，可修复性较高。

---

## 严重问题（必须修复）

### 1. 【总纲 §2】依赖关系图方向错误

- **位置：** 总纲 §2 依赖关系图 vs phase1 §3 设计目标 vs phase2 §4.1
- **问题：** 图中箭头 `llm-api-adapter → llm-harness-types` 暗示 types crate 依赖 adapter；但 phase1 明确声明 types 是零 IO 的纯类型层，不依赖 llm-api-adapter；phase2 §4.1 说明 ConvertToLlmHook 放在 loop crate 正是因为 types 不应依赖 llm-api-adapter。三者直接矛盾。
- **修复：** 将箭头改为 `llm-api-adapter → llm-harness-loop`。

---

### 2. 【phase1 / phase2】`AgentEvent::MessageStart` 字段定义直接矛盾

- **位置：** phase1 §3.3 vs phase2 §4.8 映射表
- **问题：** phase1 定义为 `MessageStart { message_id: String }`（1 个字段），phase2 映射表写的是 `AgentEvent::MessageStart { id, model, provider, api }`（4 个字段，字段名也不同）。两处定义无法同时为真。
- **修复：** 选定一处为权威定义（建议 phase1），修正 phase2 保持一致。若要透传 `model / provider / api`，需在 phase1 的 AgentEvent 定义中明确添加这些字段。

---

### 3. 【phase7】`HarnessState` 结构体与 `run_loop` 伪代码自相矛盾

- **位置：** phase7 §5.6 结构体定义 vs 正文说明 vs run_loop 伪代码
- **问题：** `HarnessState` 结构体中无 `error_message` 字段；正文明确写「Harness 通过事件的 error 处理来追踪，不需要独立字段」；但 run_loop 伪代码中直接赋值 `self.state.lock().unwrap().error_message = Some(err.to_string())`。三者自相矛盾。
- **修复：** 二选一：（a）在 HarnessState 结构体中添加 `error_message: Option<String>` 字段并删除正文的否定说明；或（b）从伪代码中删除该赋值行，改为仅 emit 事件。

---

### 4. 【phase2 / phase7】`HookedTool.assistant_message` 时序矛盾

- **位置：** phase2 结构体定义 vs phase7 run_loop 伪代码
- **问题：** `HookedTool` 要求 `assistant_message: Arc<AssistantMessage>` 由 Harness 在 turn 开始注入，但 phase7 的 run_loop 伪代码显示 `HookedTool` 在 `build_loop_config()` 中创建（`agent_loop` 调用之前），此时 `AssistantMessage` 根本不存在——它在 loop 内部 `MessageEnd` 时才构造。设计未说明如何 per-turn 更新这个字段。
- **修复：** 明确延迟注入机制。可行方案：
  - 将字段类型改为 `Arc<RwLock<Option<AssistantMessage>>>`，在 `MessageEnd` 时由 Harness 填充；
  - 或将 HookedTool 的构建时机改为 per-LLM-call（`MessageEnd` 后、工具执行前重建）。

---

### 5. 【phase1 §3.6】`AfterToolCallHook` 丢失 TS 的结果修补能力

- **位置：** phase1 §3.6 AfterToolCallHook
- **问题：** TS 版 `afterToolCall` hook 可返回 `{ content, details, isError, terminate }` 来覆盖 tool 执行结果，是用于审计缓存、结果清理的关键扩展点。Rust 版 `on_complete()` 返回 `()`，注释说「After hook 是观察者——它不能改变 tool 的执行结果」。这不是 Rust 惯用法适配，而是丢失了有意义的行为能力。
- **修复：** 将返回类型改为支持修补的类型：

  ```rust
  pub enum AfterToolCallDecision {
      Passthrough,
      Patch(ToolResultPatch),
  }

  pub struct ToolResultPatch {
      pub content:    Option<ToolResultContent>,
      pub details:    Option<serde_json::Value>,
      pub is_error:   Option<bool>,
      pub terminate:  Option<bool>,
  }
  ```

---

### 6. 【phase5】`prepare_compaction()` 的 cut point 验证算法完全缺失

- **位置：** phase5 §5.4 prepare_compaction()
- **问题：** 文档说「验证 cut point 落在有效的 entry 边界上（不能是 toolResult 中间）」，但完全未说明具体算法：cut point 应落在哪种 entry 类型之前才算有效？turn 边界如何定义？`split_turn_prefix` 字段何时触发、如何独立生成摘要、与主摘要如何合并——步骤 7 和 8 的具体算法完全缺失。
- **修复：** 明确 turn 边界定义（建议：turn = 一条 AssistantMessage + 其所有 ToolResultMessage），给出 cut point 合法性判断的完整规则，补充 `split_turn_prefix` 的处理算法（如何独立摘要后合并）。

---

### 7. 【phase5】`compact()` 摘要 prompt 模板缺失

- **位置：** phase5 §5.4 compact()
- **问题：** 文档提到 `SUMMARIZATION_SYSTEM_PROMPT` 是框架内置的，却未给出任何内容或结构。实现者需要自行决定 system prompt 内容、初始摘要与迭代更新摘要的结构差异、`path_entries` 序列化为 LLM 输入的格式（原始 JSON 还是 Markdown）、文件操作列表的附加格式。这是 compaction 功能正确性的核心。
- **修复：** 提供 `SUMMARIZATION_SYSTEM_PROMPT` 的内容模板（哪怕是草稿），明确初始 vs 迭代更新两种模式的结构差异，以及被压缩消息的序列化格式约定。

---

### 8. 【phase5】`compact()` 失败时无回滚语义说明

- **位置：** phase5 §5.4 compact() 失败处理
- **问题：** 文档描述了两段式设计的「原子性」好处，但 `compact()` 本身在 LLM 调用中途失败时（如网络超时），session 状态如何？`CompactionEntry` 是否已部分写入？phase7 的 run_loop 伪代码中也没有 `compact()` 失败的处理路径。实现者无法确定错误边界。
- **修复：** 明确写入 `CompactionEntry` 的时机（`compact()` 内部写还是调用方拿到 `CompactionResult` 后写），以及 LLM 调用失败时 session 的状态保证（不写入 / 部分写入 / 回滚）。

---

### 9. 【phase4】`build_context()` 完整算法未给出

- **位置：** phase4 §5.3 build_context()
- **问题：** 文档仅说「解释 Compaction 跳过历史，提取最后已知配置」，但实现者面临关键歧义：遇到 `CompactionEntry` 时如何定位 `first_kept_entry`？`CompactionSummaryMessage` 插入到 messages 列表哪个位置？路径上有多个 `CompactionEntry` 时只使用最新的还是全部处理？`BranchSwitch`/`BranchPoint` 等非 Message entry 如何处理？
- **修复：** 为 `build_context()` 提供完整伪代码，覆盖多个 `CompactionEntry` 的链式处理规则、非 Message payload 的跳过规则、`CompactionSummaryMessage` 的插入位置。

---

### 10. 【phase7】`run_loop` 伪代码两处关键遗漏

- **位置：** phase7 §5.6 run_loop() 事件处理伪代码
- **问题：**
  1. `initial_messages`（包含 UserMessage 等）在伪代码 step 2 合并进 ctx，但全程没有 `push_pending_write` 调用——这些消息永远不会落盘到 session，是静默的数据丢失。
  2. `BeforeTurnHook` 和 `AfterTurnHook` 的调用时机完全缺失：`TurnStart` 事件处理中只有 `emit`，没有 `before_turn` hook 调用；`TurnEnd` 同样缺失 `after_turn` hook 调用。
- **修复：** 补充：
  - `AgentStart` 事件收到时将 `initial_messages` 写入 `pending_session_writes`；
  - `TurnStart` 对应调用 `before_turn` hook；
  - `TurnEnd` 对应调用 `after_turn` hook（明确在 `flush_pending_writes` 之前还是之后）。

---

## 警告（建议修复）

### phase1-types.md

- **`AgentEvent::ThinkingDelta` 字段不一致：** phase1 定义含 `message_id` 无 `signature`；phase2 映射表含 `signature` 无 `message_id`。
- **`AgentEvent::ToolCallArgsDelta` 字段名不一致：** phase1 用 `tool_use_id / partial_input`，phase2 映射表用 `id / delta`。
- **`AgentEvent::ToolCallEnd` 字段不一致：** phase1 定义含 `tool_use_id + args` 两字段，phase2 映射表只有 `id`，丢失了 `args`。
- **hook 计数错误：** §3.6 开头写「共 13 个」，实际本节只定义 11 个，另外 2 个（`ConvertToLlmHook` / `CustomMessageConverter`）在 loop crate。
- **`AgentEvent::MessageStart` 透传路径未说明：** `StreamEvent::MessageStart` 包含 `model / provider / api` 字段，但 `AgentEvent::MessageStart` 只有 `message_id`，这些字段如何到达 `AssistantMessage` 未说明。
- **`ToolResult.details` 持久化约定缺失：** 未说明 `details` 字段是否写入 session log，影响 session 回放时的行为。
- **`NextTurnDirective.tools` 与 `active_tools` 同时非 None 时的优先级未说明。**

### phase2-loop.md

- **auth 来源描述错误：** §4.2 末尾写「auth 直接复制」暗示来自 HarnessHooks，但 phase7 的 `HarnessHooks` 结构体中没有 `auth` 字段——`auth` 是 `AgentHarness` 的独立字段。
- **steer/follow-up channel 容量未指定：** 文档提到「默认值如 32」但 `LoopConfig` 字段说明未提及；channel 满时 `steer()` 的行为（阻塞 vs drop）未说明。
- **`agent_loop_continue` steer 消息处理策略未说明：** 若 steering channel 在 loop 启动前已有消息，`agent_loop_continue` 应跳过还是注入？未明确，可能导致 steer 消息双重注入。

### phase3-agent.md

- **`continue_run` 用途定位与 phase7 矛盾：** phase3 说「Harness 调用 continue_run」，phase7 明确说 Harness 直接调用 `agent_loop()`，应改为「Harness 调用 `agent_loop_continue()`」。
- **`abort()` 的 `CancellationToken` 传播路径未说明：** abort 时是否等待正在执行的 tool 完成？`ToolContext.abort` 是同一个 token 还是派生 token？loop 何时退出？
- **`clear_all_queues()` 语义不明确：** 是否包含 `queued_next_turn`？`clear_next_turn_queue` 方法未列出。

### phase4-session.md

- **JSONL 懒加载策略描述模糊：** 「大文件（>10k entries）通过 `fs::Metadata::len` 触发懒加载策略」无法指导实现，懒加载策略的具体含义（分页/流式/不缓存）未说明。
- **`find_entries_by_type` 的 `kind` 参数字符串映射未定义：** `&str` 类型参数与 `SessionEntryPayload` 枚举的映射规则（如 `"Compaction"` → `SessionEntryPayload::Compaction`）未给出，建议改用 `SessionEntryKind` 枚举参数。

### phase5-compaction.md

- **`split_turn` 场景下两次摘要调用是并行还是串行未说明：** TS 使用 `Promise.all()` 并行调用；Rust 文档未明确，串行实现会有不必要的延迟。

### phase6-skills.md

- **`invoke_template` 的引号解析责任方存在矛盾：** 标题写「shell-style 引号解析」，正文又写「调用方负责解析引号」，两者矛盾，需明确框架内部还是调用方负责。
- **SKILL.md frontmatter 规范完全缺失：** 字段名（snake_case 还是 camelCase）、必填字段（name、description）、content 起始位置均未定义。
- **`load_prompt_templates` 目录扫描规则未说明：** 模板文件命名约定、模板名称来源（文件名 vs frontmatter）、与 `load_skills` 的算法差异均未说明。

### phase7-agent-harness.md

- **多模态 steer/follow-up 方法缺失：** phase3 列出了 `steer_message(msg: AgentMessage)` / `follow_up_message(msg: AgentMessage)`，但 Harness API 列表中无对应方法。
- **`next_turn_message` 多模态版本未列出：** 只有 `next_turn(text: impl Into<String>)`，未列出接受 `AgentMessage` 的版本。
- **`steer()` 在 Idle 阶段的 phase 约束被移除：** TS 版 Idle 时调用 steer 会抛 `invalid_state`；Rust 版分类为「任何阶段均安全」，移除了运行时保护——Idle 时积累的 steer 消息会在下次 `prompt()` 启动时意外注入。
- **`BeforeAgentStartEvent` 对应 hook 缺失：** TS 版有 `BeforeAgentStartEvent`（可修改 initial_messages / systemPrompt），Rust 版的 `HarnessHooks` 中无对应 `BeforeRunHook`。
- **`harness.continue_run()` 是否公开 API 未说明：** Agent 有 `continue_run()`，但 AgentHarness 的操作 API 列表中无对应方法，不明确是内部机制还是公开 API。
- **运行时配置写入与实际生效轮次不一致问题：** Turning 阶段调用 `set_model()` 时，配置变更的 session entry 在 turn 结束时 flush，但当前 turn 实际用的是旧 model——session replay 时会产生配置与实际执行轮次错位，文档未说明如何处理。
- **`abort()` 行为规格不完整：** 未说明 abort 时是否清空 steer/follow-up queue，以及是否等待 loop 停止后才发出 `Aborted` 事件。

---

## 改进建议（可选）

- **统一 channel 容量参数：** 推荐在 `Agent::new()` 和 `AgentHarness::new()` 的参数中提供 `queue_capacity: usize`（默认 32），并说明 channel 满时的行为（建议：steer/follow-up 满时 drop 并 warn，不阻塞调用方）。
- **`find_entries_by_type` 参数类型：** 将 `&str` 参数改为 `SessionEntryKind` 枚举（实现 `Display / AsRef<str>`），消除字符串映射的歧义。
- **补充 `BeforeRunHook`：** 在 `HarnessHooks` 中增加 `before_run: Option<Arc<dyn BeforeRunHook>>`，在 `initial_messages` 传入 `agent_loop` 之前调用，允许修改消息或 system prompt，还原 TS `BeforeAgentStartEvent` 的能力。
- **总纲补充文字说明：** 依赖关系图修正后，增加一段简短说明各 crate 的边界职责（纯类型层 / loop 逻辑层 / harness 层），帮助读者快速建立全局认知。

---

## 各维度结论

| 维度 | 结论 | 主要发现 |
|---|---|---|
| 完整性 | 有警告 | 12 条警告，无严重问题。主要缺失：多模态 API 变体（steer_message / follow_up_message / next_turn_message）、`harness.abort()` 完整规格、`BeforeAgentStartEvent` 对应 hook、引号解析责任方矛盾 |
| 设计忠实度 | 有警告（含 1 条严重） | `AfterToolCallHook` 丢失 TS 结果修补能力（严重）；`steer()` Idle 阶段 phase 约束被移除、`BeforeAgentStartEvent` hook 缺失、split_turn 并行调用未明确（警告） |
| 内部一致性 | 有严重问题 | 4 条严重：依赖图方向错误、`AgentEvent::MessageStart` 跨文件字段定义矛盾、`HarnessState.error_message` 正文/结构体/伪代码三处矛盾、`HookedTool.assistant_message` 时序矛盾；另有 6 条警告 |
| 实现指导充分性 | 有严重问题 | 5 条严重：compaction cut point 算法缺失、prompt 模板缺失、失败回滚语义缺失、`build_context()` 算法不完整、`run_loop` 关键写入遗漏；另 9 条警告 |

---

## 优先处理顺序

1. **内部一致性矛盾**（实现者会写出不同理解的代码，且难以测试发现）
   - 依赖关系图方向错误
   - `AgentEvent` 字段定义跨文件矛盾（MessageStart / ThinkingDelta / ToolCallArgsDelta / ToolCallEnd）
   - `HarnessState.error_message` 三处矛盾
   - `HookedTool.assistant_message` 时序矛盾

2. **run_loop 关键写入遗漏**（session 数据会静默丢失，运行时才会发现）
   - initial_messages 未落盘
   - before_turn / after_turn hook 调用缺失

3. **功能性退化**
   - `AfterToolCallHook` 结果修补能力

4. **算法细节补全**（可在实现阶段边写边补，但建议先在 spec 中明确）
   - compaction cut point 验证算法
   - `build_context()` 完整伪代码
   - compact() 失败回滚语义

---

## 附录：主 agent 对审查报告的逐条核验（2026-06-07）

> 以下是对照当前 v7 设计文档逐条核验后的判断。每条给出：成立/部分成立/不成立 + 理由。

### 严重问题（10 条）

**#1 依赖关系图方向错误 — ✅ 成立。**
总纲 §2 的箭头方向确实画反了。types 是零 IO 纯类型层，不依赖 llm-api-adapter。正确方向应为 `llm-api-adapter → llm-harness-loop`。

**#2 AgentEvent::MessageStart 字段矛盾 — ⚠️ 部分成立。**
审查报告引用的 `phase2 §4.8 映射表` 在当前版本中不存在（当前 phase2 只到 §4.7），"两处定义字段数不同" 的具体指控不准确。但**问题本质存在**：`StreamEvent::MessageStart` 携带 `{id, model, provider, api}`，`AgentEvent::MessageStart` 只有 `message_id`——provider 元数据如何从 StreamEvent 流入最终的 `AssistantMessage`（`AssistantMessage` 有 `provider`/`api`/`model` 字段）全程没有说明。不是跨文件矛盾，而是**数据流断裂**。建议在 phase2 补充 StreamEvent → AgentEvent 的完整映射表，明确每个 StreamEvent 变体对应哪个 AgentEvent 变体以及字段如何提取。

**#3 HarnessState.error_message 三处矛盾 — ✅ 成立。**
正文说不需要，结构体没定义，伪代码却直接赋值。三者互相否定。建议：在 `HarnessState` 结构体中添加 `error_message: Option<String>` 字段，删除正文中的否定说明，保留伪代码中的赋值逻辑。

**#4 HookedTool.assistant_message 时序矛盾 — ✅ 成立。**
`HookedTool` 在 `build_loop_config()` 中构造（agent_loop 调用前），但 `assistant_message: Arc<AssistantMessage>` 在 loop 内部的 `MessageEnd` 时才产生。构造时注入一个还不存在的值不可能。审查报告建议的 `Arc<RwLock<Option<AssistantMessage>>>` 是可行的延迟注入方案。替代方案：将 HookedTool 的构建推迟到 per-LLM-call（MessageEnd 后、工具执行前），由 Harness 在收到 MessageEnd 事件后重建工具列表。替代方案不需要 RwLock，但需要 Harness 介入 loop 内部流程——接口更复杂。

**#5 AfterToolCallHook 丢失结果修补能力 — ✅ 成立。**
TS 的 `afterToolCall` 可以覆盖 `content/details/isError/terminate`，Rust 的 `on_complete() → ()` 是纯观察者。审查报告提出的 `AfterToolCallDecision::Patch(ToolResultPatch)` 修复方案合理，建议采纳。

**#6 prepare_compaction cut point 验证算法缺失 — ✅ 成立。**
文档只有一句 "验证 cut point 落在有效的 entry 边界上"，没有具体规则。定义 turn 边界 = 一条 UserMessage + 一条 AssistantMessage + 其所有 ToolResultMessage，则 cut point 合法位置为：UserMessage 之前、或 CompactionEntry 之后、或 BranchSummary 之后。toolResult 之前截断是不合法的（LLM 会看到孤立的 tool result 没有对应的 assistant prompt）。

**#7 compact() 摘要 prompt 模板缺失 — ✅ 成立。**
TS 源码中 `SUMMARIZATION_PROMPT` 和 `UPDATE_SUMMARIZATION_PROMPT` 有完整的文本模板（约 50 行），Rust 设计应该附上——哪怕标注为 "参考 TS 版本，最终 prompt 在实现阶段调优"。

**#8 compact() 失败回滚语义缺失 — ✅ 成立。**
建议明确：`compact()` 内部**不**写入 session——它只返回 `CompactionResult`。调用方（Harness）拿到 `Result::Ok(CompactionResult)` 后才调用 `session.append(SessionEntryPayload::Compaction(...))`。如果 `compact()` 返回 `Err`，session 完全未被修改。这避免了回滚问题。

**#9 build_context() 完整算法缺失 — ✅ 成立。**
应该补伪代码，至少覆盖：(1) 从 active_cursor 调用 `path_to_root` 获取原始路径；(2) 从 root 向 leaf 遍历；(3) 遇到 CompactionEntry 时，记录其 `first_kept_entry`，后续只处理 id ≥ first_kept_entry 的 entry；(4) 非 Message payload 跳过；(5) 最后一个 CompactionEntry 的 summary_message 插入到消息列表最前面；(6) 沿途提取最后一次 ModelChange/ThinkingLevelChange/ActiveToolsChange 的值。

**#10 run_loop 伪代码两处遗漏 — ✅ 成立。**
- initial_messages 未落盘：`AgentStart` 事件携带 `initial_messages`，Harness 应将它们写入 `pending_session_writes`（在收到 AgentStart 或第一个 TurnStart 时）。
- before_turn/after_turn hook 未调用：应在 TurnStart 时调用 `before_turn`，在 TurnEnd 的 `flush_pending_writes` 之前调用 `after_turn`。

### 警告（精选核验）

| 警告 | 判断 | 理由 |
|---|---|---|
| AgentEvent::ThinkingDelta 缺 signature | ✅ | Anthropic 的 thinking delta 确实携带 content signature，当前定义不含此字段 |
| NextTurnDirective tools 与 active_tools 同时非 None 优先级 | ✅ | 应明确 tools 替换全集（忽略 active_tools），或 active_tools 先过滤再替换 |
| continue_run 用途描述与 phase7 矛盾 | ✅ | phase3 写 "AgentHarness 的 prepare_next_turn 依赖 continue_run"，但 phase7 说 Harness 直接调用 agent_loop_continue()——不是 Agent::continue_run() |
| find_entries_by_type 参数建议用枚举 | ✅ | `&str` 参数无编译期保证，建议加 `SessionEntryKind` 枚举 |
| steer() Idle 阶段 phase 约束被移除 | ✅ | TS 中 Idle 时 steer 抛错；Rust 允许但会在下次 prompt 时意外注入。应加 runtime 检查或文档化 |
| 多模态 steer/follow-up API 缺失 | ✅ | Agent 有 steer_message()，Harness API 列表无对应方法——应该补齐 |
| JSONL 懒加载策略描述模糊 | ✅ | "大文件触发懒加载" 无法指导实现，建议改为 "内存缓存上限为 N 条 entry，超出后改为按需从文件读取" |
| invoke_template 引号解析责任方矛盾 | ✅ | 文档先说 "shell-style 引号解析"，后说 "调用方负责解析"——需统一 |
| split_turn 场景下两次 LLM 调用是否并行 | ✅ | TS 用 Promise.all() 并行；Rust 文档未说明，建议明确并行（两次摘要调用独立） |
| auth 来源描述错误 | ✅ | phase2 说 "auth 直接复制（自 HarnessHooks）"，但 HarnessHooks 没有 auth 字段——auth 是 AgentHarness 的独立字段 |

### 对审查报告本身的评价

**总体质量：高。** 10 条严重问题全部指向真实问题（#2 需要修正引用细节，但本质问题存在）。约 20 条警告中绝大多数成立。审查报告对四份 phase 文档 + 总纲之间的**跨文件交叉引用矛盾**抓得非常准——这正是单文件审查容易漏掉、但对实现者影响最大的问题类别。

**一个小修正：** #2 引用的 `phase2 §4.8 映射表` 在当前版本中不存在。建议改为：phase2 §4 开头的 `StreamEvent` 定义与 phase1 §3.3 的 `AgentEvent` 定义之间缺少数据流映射说明。

---

## 补充意见（2026-06-07）

### #4 HookedTool.assistant_message 时序矛盾——选定修复方案

审查报告提出两个候选方案，逐一分析后选定**第三条路：将 `assistant_message` 从 `HookedTool` 结构体移入 `ToolContext`**，原因如下：

**方案 A（`Arc<RwLock<Option<AssistantMessage>>>`）的隐患：**
HookedTool 在 `build_loop_config()` 中创建后被 move 进 LoopConfig，Harness 之后没有 HookedTool 的引用。若用 RwLock，Harness 必须在构造时额外维护一个 `Vec<Arc<RwLock<...>>>` 与 HookedTool 一一对应，专门用来在 `MessageEnd` 时写入值。这是为了解决一个设计问题而引入的管理负担，而非真正消除问题。

**方案 B（per-LLM-call 重建 HookedTool）的隐患：**
要求 loop 在每次 `MessageEnd` 后暂停，通知 Harness 重建工具列表再继续——打破了 `agent_loop()` 的纯函数特性，接口复杂度上升。

**选定方案：`assistant_message` 移入 `ToolContext`**

根本原因是 `assistant_message` 不是工具的**属性**，而是本次工具调用的**上下文**。将它放在 `HookedTool` 是职责错位。正确位置是 `ToolContext`：

```rust
pub struct ToolContext {
    pub tool_use_id:       String,
    pub turn_index:        u32,
    pub assistant_message: Arc<AssistantMessage>,   // 触发本次工具调用的 LLM 响应
    pub abort:             CancellationToken,
    // ...
}
```

Loop 在 `MessageEnd` 后已持有完整的 `AssistantMessage`（刚刚构造完），构造 `ToolContext` 时直接传入。`BeforeToolCallCtx` 和 `AfterToolCallCtx` 通过 `ToolContext` 拿到它。

`HookedTool` 结构体因此可以去掉 `assistant_message` 和 `turn_index` 字段（两者都移入 `ToolContext`），变回无状态的纯 decorator：

```rust
pub struct HookedTool {
    pub inner:  Arc<dyn Tool>,
    pub before: Option<Arc<dyn BeforeToolCallHook>>,
    pub after:  Option<Arc<dyn AfterToolCallHook>>,
}
```

**结论：**
- 无共享可变状态，无 Arc 备份管理负担
- HookedTool 在 loop 启动时一次性构建，无需 per-turn 重建
- `agent_loop()` 保持纯函数特性
- Hook 实现者通过 `ctx.assistant_message` 拿到触发消息，语义比「存在 HookedTool 上」更自然

此方案需同步修改 phase1 §3.6 的 `HookedTool` 结构体定义、phase2 §4.3 的 HookedTool 说明、以及 `ToolContext` 的字段定义（phase1 §3.5）。

### 其他补充

**#2 数据流断裂的具体位置：** `StreamEvent::MessageStart { id, model, provider, api }` 触发时，loop 将这四个字段存入 `StreamingState`（§4.8 中已定义）。`MessageEnd` 时，`StreamingState.model / provider / api` 被写入 `AssistantMessage` 的对应字段。因此 `AgentEvent::MessageStart` 不需要携带 `model / provider / api`——它们最终通过 `MessageEnd { message }` 里的 `AssistantMessage` 到达调用方。这条数据流在 §4.8 有隐含描述，但没有在 phase1 的 AgentEvent 注释中明确说明，建议在 phase1 §3.3 的 `MessageStart` variant 旁加一行注释解释这个设计意图。

**#8 修复方案的架构含义：** `compact()` 不写 session，只返回 `CompactionResult`，Harness 在 `Ok(result)` 分支调用 `session.append(SessionEntryPayload::Compaction(...))` 这一选择同时解决了另一个问题：Harness 是整个设计中 session 的**唯一写入方**（通过 `pending_session_writes` + `flush_pending_writes`）。若 `compact()` 内部写 session，则违反了这一原则。从设计一致性角度看，`compact()` 不写 session 不仅是工程选择，也是架构约束。建议在 phase5 §5.4 的设计理由中明确写出这一约束。
