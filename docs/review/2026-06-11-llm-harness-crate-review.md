# llm-harness Crate Code Review

**Date**: 2026-06-11  
**Scope**: `crates/llm-harness/src/` 全量实现（`agent.rs`、`harness.rs`、`compaction.rs`、`skills.rs`、`session/` 下全部模块）  
**Spec**: `docs/superpowers/specs/` phase3–7  
**结论**: NEEDS_CHANGES

---

## 正式问题

### P0（必须修复）

#### P0-1: `run_loop` 错误路径绕过状态清理，Harness 永久卡死

**维度**: 健壮性  
**位置**: `harness.rs:1179–1355`（关键路径：line 1191、1194、1325）

`run_loop` 在 `set_phase(Turning)`（line 1191）之后，step-5 清理块（line 1347–1353）和 `set_phase(Idle)` 位于 while 循环之外。`build_context().await?`（line 1194）或 `flush_pending_writes().await?`（line 1325）一旦报错，函数通过 `?` 提前返回，清理代码不执行。

后果：phase 永久停在 `Turning`；所有后续 `prompt()`/`compact()` 等操作均返回 `HarnessError::NotIdle`；`wait_for_settled()` 永久挂起；`current_abort` 中残留旧 token。

**根因分析**：P0-1 包含两个正交的子问题：

1. **Phase 卡死**：清理代码不在所有出口执行
2. **flush 失败时数据丢失**：`flush_pending_writes` 先 `std::mem::take` 清空内存，再逐条写入；写到第 k 条失败时，第 k…n 条从内存消失、也未写入 session，数据两头都没有

**修复一：保证清理代码必然执行**

将 `run_loop` 拆成 drive（可失败）+ cleanup（必然执行）：

```rust
async fn run_loop(&self, initial: Vec<AgentMessage>) -> Result<(), HarnessError> {
    // setup：建 channels、创 abort token、设置 system_prompt
    let (steer_rx, follow_up_rx, abort, system_prompt) = {
        let mut inner = self.inner.lock().unwrap();
        inner.state.streaming_message = None;
        inner.state.pending_tool_calls.clear();
        // ... 其余 setup ...
    };
    self.set_phase(HarnessPhase::Turning);

    let result = self
        .drive_loop(initial, steer_rx, follow_up_rx, abort, system_prompt)
        .await;

    // cleanup：无论 drive_loop 成功还是失败都执行
    {
        let mut inner = self.inner.lock().unwrap();
        inner.state.streaming_message = None;
        inner.state.pending_tool_calls.clear();
        inner.current_abort = None;
        if let Err(ref e) = result {
            inner.state.error_message = Some(e.to_string());
            // 清掉未能落盘的 pending，避免污染下一次 run
            inner.state.pending_session_writes.clear();
        }
    }
    self.set_phase(HarnessPhase::Idle);
    result
}
```

`drive_loop` 即现在 `run_loop` 里 `build_context` 至 `while let Some(event)` 的全部内容，接收 setup 阶段准备好的参数。

**修复二：缩小 `flush_pending_writes` 的数据丢失窗口**

把"全取走再写"改为"写一条、移一条"——写入成功后再从 pending 移除：

```rust
async fn flush_pending_writes(&self) -> Result<usize, HarnessError> {
    let mut count = 0;
    loop {
        // peek 第一条，不从内存移除
        let payload = {
            let inner = self.inner.lock().unwrap();
            inner.state.pending_session_writes.first().cloned()
        };
        let Some(payload) = payload else { break };

        // 写入失败 → payload 仍在 pending_session_writes
        // run_loop cleanup 在 Err 分支会统一清空 pending
        self.session.append(payload).await?;

        // 写入成功 → 从 pending 移除
        self.inner
            .lock()
            .unwrap()
            .state
            .pending_session_writes
            .remove(0);
        count += 1;
    }
    Ok(count)
}
```

注：`remove(0)` 为 O(n)，pending 条目通常极少（每个 turn 数条），影响可忽略。若未来条目量增大，可将 `pending_session_writes` 从 `Vec` 改为 `VecDeque` 并用 `pop_front()`，属独立优化。

**两种失败路径下的行为（修复后）**

| 出错点 | 已写入 session？ | 修复后状态 | 是否可安全重试 |
|--------|----------------|-----------|--------------|
| `build_context()` line 1194 | 否 | Idle，pending 为空，`error_message` 已记录 | ✅ 可直接重试 `prompt()` |
| `flush_pending_writes()` line 1325，第 k 条失败 | 前 k-1 条已写入 | Idle，第 k…n 条被 cleanup 清空，`error_message` 已记录 | ⚠️ session 存在部分写入，调用方应检视后决定重试或重置 |

与原始实现相比，修复后**失败边界是明确的**：`build_context` 失败时 session 完全未修改；`flush` 失败时可从 `error_message` 得知是存储层问题，并能通过 `build_context()` 检视当前 session 实际状态，而不是所有 pending 数据悄无声息地消失。

---

#### P0-2: `AgentHarnessEvent::ToolCallEnd` 的 `tool_name` 字段硬编码为空字符串

**维度**: 需求/设计符合度  
**位置**: `harness.rs:~1307`

`ToolCallEnd` 事件中 `tool_name: String::new()`，订阅者无法从该事件获取工具名称。代码注释 `// name not in ToolExecutionEnd` 自承问题。`ToolCallStart`（line ~1262）已正确携带 `tool_name`，形成不对称的事件对。Spec phase7 要求 `ToolCallEnd` 携带 `tool_name`。

**修复建议**：在 `HarnessInner` 中添加 `active_tool_names: HashMap<String, String>`；在处理 `ToolExecutionStart` 时插入 `tool_use_id → tool_name`，在 `ToolExecutionEnd` 时取出并填充事件。

---

#### P0-3 ~ P0-13: 大量 pub 类型/字段/variant 缺少 `///` doc comment（共 11 处）

**维度**: 工程规范  
**规则来源**: CLAUDE.md "实现后自查 → ❌ 所有 pub 类型、字段、enum variant 是否有 `///` doc comment"

| 位置 | 违规项 |
|------|--------|
| `session/types.rs:201–213` | `SessionEntryKind` 11 个 variant 无 `///` |
| `session/types.rs:10–49` | `SessionEntryPayload` named-variant 字段（`to`/`provider`/`model_id`/`name`/`from`/`label`/`summary`/`custom_type`/`data`）无 `///`（注：variant 本身有 `///`，字段级注释对 enum variant 字段非 Rust 惯例，此处按 CLAUDE.md 严格解读仍视作缺失） |
| `session/types.rs:118–125` | `CreateSessionOptions` 4 个字段无 `///` |
| `session/types.rs:127–133` | `ListSessionOptions` 4 个字段无 `///` |
| `session/types.rs:136–143` | `ListOrder` 4 个 variant 无 `///` |
| `session/types.rs:147–151` | `ForkOptions.name` 无 `///`（`copy_entries` 已有 `///`） |
| `harness.rs:130–202` | `AgentHarnessEvent` 所有 struct-variant 字段（`from`/`to`/`added`/`removed`/`leaf` 等）无 `///` |
| `session/repo.rs:16–38` | `SessionRepo` 4 个 trait 方法（`create`/`open`/`list`/`delete`）无 `///`（`fork` 已有 `///`） |
| `session/storage.rs:15–65` | `SessionStorage` 11 个方法无 `///` |
| `session/repo.rs:52` | `InMemorySessionRepo::new` 无 `///`（struct 已有 `///`） |
| `session/storage.rs:82` | `InMemorySessionStorage::new` 无 `///`（struct 已有 `///`） |
| `session/session.rs:19` | `Session::new` 无 `///` |

---

### P1（应该修复）

#### P1-1: `Agent::clear_steering_queue` / `clear_follow_up_queue` 方法体为空，调用无效

**维度**: 健壮性  
**位置**: `agent.rs:287–299`

两个方法体仅含注释，无实际操作。Idle 状态下已缓冲在 channel 中的消息不会被清除，API 文档契约违反。对比 `AgentHarness` 同名方法（line 917–928）通过替换 sender channel 正确实现了清除。

**修复建议**：

```rust
pub fn clear_steering_queue(&self) {
    let mut inner = self.inner.lock().unwrap();
    let (tx, _) = mpsc::channel(inner.queue_capacity);
    inner.steer_tx = tx;
}
```

---

#### P1-2: `Agent::prompt_with_messages` 传入单条 UserMessage 时走 append 路径，违反"替换"文档

**维度**: 健壮性 + 需求符合度  
**位置**: `agent.rs:375–380`

启发式 `if initial.len() == 1 && matches!(initial[0], User(_))` 无法区分来自 `prompt()` 的调用（应追加）和来自 `prompt_with_messages()` 的调用（应替换）。方法文档承诺"replacing the current transcript"，但单条 User 消息时实际执行 `extend`。

**修复建议**：给 `run_with_initial` 增加 `replace_transcript: bool` 参数，由 `prompt()` 传 `false`，由 `prompt_with_messages()` 传 `true`，消除启发式判断。

---

#### P1-3: `AgentHarness::append_message` / `append_custom_entry` 无 phase 检查，Running 阶段调用损坏 session 树

**维度**: 契约与信任链  
**位置**: `harness.rs:719–733`

两个方法直接调用 `session.append()`，推进 `active_cursor`，无 phase 检查。在 `HarnessPhase::Turning` 时并发调用会使 `flush_pending_writes` 将 pending 条目挂到外部注入条目的子节点位置，而非预期的 AssistantMessage 同级节点，损坏 turn 树结构，且持久化到存储中。

CLAUDE.md 明确："Harness 是 session 的唯一写入方——任何需要落盘的操作都通过 `pending_session_writes` + `flush_pending_writes` 路径。"

**修复建议**：添加 Idle-only 检查（与 `set_session_name`、branch 操作保持一致）：

```rust
pub async fn append_message(&self, msg: AgentMessage) -> Result<EntryId, HarnessError> {
    {
        let inner = self.inner.lock().unwrap();
        if inner.state.phase != HarnessPhase::Idle {
            return Err(HarnessError::NotIdle(inner.state.phase));
        }
    }
    Ok(self.session.append_message(msg).await?)
}
```

---

#### P1-4: `before_compact` hook 的 `Override` 路径不校验 `first_kept_entry`，无效 ID 导致下轮 LLM 上下文翻倍

**维度**: 契约与信任链  
**位置**: `harness.rs:1388–1393`（`apply_compaction_result` line 1447–1450）

`Override(result)` 直接进入 `apply_compaction_result`，`result.first_kept_entry` 未验证是否存在于 `path` 中。无效 ID 写入 session 成为悬空锚点；后续 `build_context_from_entries` 的 `unwrap_or(0)` fallback 将 `start_idx` 设为 0，导致下轮 LLM 同时收到完整历史和摘要，上下文窗口可能溢出，且无任何错误提示。

**修复建议**：在 `apply_compaction_result` 入口校验：

```rust
if !path.iter().any(|e| e.id == result.first_kept_entry) {
    return Err(CompactionError::InvalidFirstKeptEntry(result.first_kept_entry).into());
}
```

---

#### P1-5: `Session::list_branches` O(L×M) 复杂度，每个 leaf 完整克隆路径

**维度**: 性能  
**位置**: `session/session.rs:181–207`

对每个 leaf 串行调用 `path_to_root`，持锁遍历并克隆整条路径的 `SessionEntry`（含 `AgentMessage`）。L 个 leaf × 平均路径深度 M 的克隆操作在多分支会话下代价真实（分支是 first-class 功能）。

**修复建议**：一次性 BFS/DFS 遍历全树，O(N) 内同时获得所有 leaf 元数据；或增加只返回分支 metadata（不克隆 message 内容）的存储方法。

---

#### P1-6: `label_at` O(N) 全表扫描，在 `list_branches` 热路径上被调用 L 次

**维度**: 性能  
**位置**: `session/storage.rs:217–231`

`for entry in st.entries.values()` 无索引线性扫描。`InMemoryState` 已有 `children: HashMap<Option<EntryId>, Vec<EntryId>>` 索引，可将复杂度降为 O(k)，修改一行即可。

**修复建议**：

```rust
let children = st.children.get(&Some(id)).map(|v| v.as_slice()).unwrap_or(&[]);
for child_id in children {
    if let Some(e) = st.entries.get(child_id) { /* check Label payload */ }
}
```

---

### P2（建议改进）

#### P2-1: `before_compact` hook 收到的 `estimated_tokens` 为 `path.len() * 100`，与实际 token 数可能相差数量级

**维度**: 契约 + 需求符合度  
**位置**: `harness.rs:1376–1378`

`rough_tokens = path.len() * 100` 不使用 `estimate_tokens_for_entry` 逻辑，不考虑 `AssistantMessage.usage` 数据。对含大型消息的会话，实际 token 数可能是此估算值的 1000 倍。Skip/Proceed 决策 hook 收到字段名为 `estimated_tokens` 的值，语义暗示真实 token 数，导致阈值判断错误。

代码注释"Override hooks don't need accuracy"只说明 Override 场景，未文档化对 Skip/Proceed 场景的影响。

**修复建议**：调用 hook 前使用精确估算：`let estimated_tokens: usize = path.iter().map(estimate_tokens_for_entry).sum();`

---

#### P2-2: `compressed_entries` 统计字段在异常路径下错误报告 0

**维度**: 需求符合度（统计层面）  
**位置**: `harness.rs:1447–1450`

`position(...).unwrap_or(0)` 在 Override hook 提供无效 `first_kept_entry` 时返回 0，`CompactionStats.compressed_entries` 报告"压缩 0 条"，数据误导。不影响功能正确性（该场景已由 P1-4 覆盖），但统计数据错误会影响 UI 展示。

**修复建议**：P1-4 的校验修复后，`position()` 必然有值，可改为 `.expect("first_kept_entry validated above")`。

---

#### P2-3: `format_entries_as_text` 双层 String 分配

**维度**: 性能  
**位置**: `compaction.rs:350–410`

N 个 `format_message` 中间 String 推入 `Vec<String>` 再 `join`，产生 N+1 次独立分配。`content_blocks_to_text` 内部也额外 `collect::<Vec<_>>()` 再 `join`。非热路径，但可优化。

**修复建议**：使用 `String::with_capacity(estimated_size)` + `write!` 统一写入单个缓冲，消除中间 Vec 分配。

---

## Follow-up Notes

- **`ForkOptions::copy_entries: false`**：当前为 v1 已知未实现功能（spec 明确 defer 到 v1.x）。字段已有 `/// v1 forces this to `true` (full entry copy with new IDs).`，可进一步补充 v1.x 计划：建议改为 `/// v1 强制为 true（完整条目复制）；引用模式（false）推迟到 v1.x 实现。`

- **`AgentHarnessOptions::new()` 的 dummy channel**：line 273–277 创建并立即 drop 了两对临时 channel，`AgentHarness::with_session` 会重新创建正式 channel。不影响正确性，但代码令人困惑，建议后续清理。

- **`generate_branch_summary` 并发语义**：该方法在 Idle 阶段直接调用 LLM，不更新 phase，多个并发调用可以并行执行。建议在方法文档中明确说明这是否为设计意图。

- **`skills()` / `templates()` 返回 clone**：返回 `Vec<Skill>` 而非 `&[Skill]` 引用；对大型 skill 集频繁调用有额外分配，当前场景可接受。

---

## 通过验证项

- ✅ 未发现 `RwLock` 使用（项目统一使用 `Mutex` + 快照模式）
- ✅ 未发现日文/韩文注释
- ✅ `llm-harness` 未直接依赖 `llm-api-adapter`（通过 `llm-harness-loop` pub use 获取类型）
- ✅ `compact()` 函数本身不写 session，仅返回 `CompactionResult`（契约满足）
- ✅ `Session::navigate_to()` 通过 `AgentHarness::navigate_tree` 的 Idle guard 覆盖，无并发风险
- ✅ `generate_branch_summary` 的 Idle-only 直接写 session 符合豁免模式（与 `compact`/`set_session_name`/branch 操作一致）
- ✅ `fork()` 在 v1 下强制全量复制符合 spec 意图
- ✅ `set_tools` 使用 `ActiveToolsChange` 符合 spec（spec 只定义一种工具变更 entry 类型）
