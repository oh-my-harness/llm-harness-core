# llm-harness-core 设计审核意见

**日期：** 2026-06-07
**审核对象：** [2026-06-07-llm-harness-core-design.md](./2026-06-07-llm-harness-core-design.md)
**状态：** 待讨论

---

## 🔴 严重问题

### 1. `std::sync::Mutex` 在 async 上下文中 —— 死锁风险

**涉及行：** L215 `state: Arc<Mutex<AgentState>>`

`std::sync::Mutex` **严禁**在持有锁的情况下跨越 `.await` 点。Tokio 的协作式调度在同一个 worker 线程上执行多个 task——如果持有 `std::sync::Mutex` 的 task 在 `.await` 处让出执行权，同一 worker 线程上的其他 task 尝试获取同一个锁时，将导致死锁（或在 tokio 检测到后 panic）。

`Agent::prompt()` 是 async 方法，其内部几乎必然需要在持有 state 锁期间调用 `agent_loop()`，而 `agent_loop()` 返回的 `Stream` 跨越多次 `.await`。即使当前实现碰巧安全，这也是脆弱的——任何后续修改都可能在锁内引入 `.await`。

**建议方案（二选一）：**

1. **使用 `tokio::sync::Mutex`** —— 允许跨越 `.await`，但要求所有对 state 的访问都经过 `.lock().await`，且需要确保锁不被长时间持有。

2. **（推荐）将状态迁移改为 channel 传递** —— 需要跨 `.await` 的状态（如 `TurnSnapshot`）在 turn 开始前从 `Mutex` 中取出 clone，turn 期间不持锁。`std::sync::Mutex` 仅保护非 await 的快速操作（如 `steer()` 的 channel 发送）。

```rust
// 推荐模式：锁仅在同步块内持有，跨 await 的数据提前 clone
let snapshot = {
    let state = self.state.lock().unwrap();
    TurnSnapshot::from(&*state)
}; // 锁在此释放
// 后续 agent_loop 使用 snapshot，不再需要锁
```

---

### 2. 多个关键类型未定义 —— 接口不可编译

以下类型在文档中被引用但从未给出定义。这些是公开 API 的核心组件，缺少任意一个都会导致 crate 无法通过编译或接口不可用。

| 类型 | 引用位置 | 影响面 |
|---|---|---|
| `ToolError` | L113 `Tool::execute` 返回值 | 所有 Tool 实现者 |
| `AgentError` | L96 `AgentEvent::Error(AgentError)` | 所有事件消费者 |
| `StopReason` | L79 `AssistantMessage.stop_reason` | 消息模型 |
| `MessageId` | L311 `CompactionResult.compressed_ids: Vec<MessageId>` | Compaction ↔ Session 边界 |
| `AgentHarnessEvent` | L383 `subscribe()` 返回值 | AgentHarness 的所有调用方 |
| `SteerReceiver` | L171 `LoopConfig.steer_rx` | Loop 配置 |
| `FollowUpReceiver` | L172 `LoopConfig.follow_up_rx` | Loop 配置 |
| `TransformContextHook` | L165 `LoopConfig.transform_context` | Compaction 集成点 |
| `BeforeToolCallHook` | L167 `LoopConfig.before_tool_call` | 工具拦截 |
| `AfterToolCallHook` | L168 `LoopConfig.after_tool_call` | 工具拦截 |
| `ShouldStopHook` | L175 `LoopConfig.should_stop` | Loop 终止控制 |
| `BeforeTurnHook` | L361 `HarnessHooks.before_turn` | Harness 钩子 |
| `AfterTurnHook` | L362 `HarnessHooks.after_turn` | Harness 钩子 |

**特别指出：`MessageId` vs `EntryId` 的命名不一致 (L311 vs L65)。** 如果两者是同一类型，应统一命名为 `EntryId`；如果是不同类型，需说明区别。当前 `CompactionResult` 引用 session log 中的 entry，理应使用 `EntryId`。

---

### 3. Tool batch 降级语义浪费并发资源

**涉及行：** L186-191

当前设计：
> 若任意一个 tool 为 Sequential：整批退化为顺序执行

**问题：** LLM 不知道哪些 tool 被标记为 `Sequential`。它可能在同一次 `tool_use` 响应中混合调用 5 个 `Parallel` tool 和 1 个 `Sequential` tool。按当前设计，全部 6 个工具顺序执行，即使前 5 个完全可以并发。这种"连坐"降级在最坏情况下会导致延迟线性膨胀：N 个 parallel + 1 个 sequential = N+1 倍延迟。

**建议方案（二选一）：**

1. **分治执行：** 将 batch 按 `Sequential` tool 的位置拆分为子组。每个子组内 Parallel tool 并发执行；子组间顺序执行。保留 LLM 返回的调用顺序。

```
LLM 返回: [P1, P2, S1, P3, P4, S2, P5]

执行方案:
  子组1: join_all(P1, P2)
  → 子组2: 单执行 S1
  → 子组3: join_all(P3, P4)
  → 子组4: 单执行 S2
  → 子组5: 单执行 P5
```

2. **若坚持全退策略，至少文档化理由和性能影响警告。** 调用方需要知道哪些 tool 是 Sequential 的，并在提示词中告知 LLM 不要混合使用。

---

## 🟡 重要问题

### 4. Session 分支模型 —— 有声明、无设计

**涉及行：** L267 "支持分支、回放、审计"

`SessionStorage` trait（L281-286）只有两个操作：

```rust
fn append(&self, entry: SessionEntry) -> BoxFuture<Result<EntryId>>;
fn read_range(&self, from: Option<EntryId>) -> BoxFuture<Result<Vec<(EntryId, SessionEntry)>>>;
```

没有任何分支原语。需要回答：

| 问题 | 当前文档状态 |
|---|---|
| 如何创建一个新分支？分支点如何引用？ | 未定义 |
| `Leaf` entry (L278) 的语义？ | "标记当前活跃分支末端"，单分支不需要 |
| `BranchSummary` (L276) 的内容？ | 未展开 |
| `JsonlSessionStorage` 如何存储分支？ | JSONL 是线性的，分支需要多文件或内联元数据 |
| 分支间如何切换？ | 未定义 |
| 多分支的 compaction 行为？ | 未讨论 |

**建议：** 要么在 v1 中定义完整的分支模型（包括分支创建、切换、合并的存储语义），要么将分支从当前范围中移除，在 `SessionEntry` 中预留 `BranchPoint` / `BranchSwitch` 枚举变体作为未来扩展点。

---

### 5. Compaction 与 Session 读取的交互未定义

**涉及行：** L293-296, L281-286

Compaction "不删除历史，而是追加一条 Compaction entry"——这是好的设计原则。但 `SessionStorage::read_range()` 的行为未定义：

- 调用方通过 `read_range` 读取 session log 时，**谁来负责跳过被压缩覆盖的旧消息**？是 storage 层自动过滤，还是调用方自行解析 `Compaction` entry 并手动跳过？
- 如果 storage 层自动跳过，它需要理解 `Compaction` entry 的 `compressed_ids` 字段——这意味着 storage 不是透明的字节追加层，而需要理解 entry 语义。
- 如果调用方手动跳过，那么 `read_range` 返回的原始数据中包含已被压缩的"过时"消息，调用方需要维护复杂度。

**建议：** 明确定义两层接口：

```rust
// 底层：原始日志，所有 entry 原样返回
fn read_range_raw(...) -> Vec<(EntryId, SessionEntry)>;

// 高层：应用 compaction 过滤，返回"当前有效视图"
fn read_range_effective(...) -> Vec<(EntryId, SessionEntry)>;
```

---

### 6. AgentHarness 阶段与 Agent 阶段的关系未澄清

**涉及行：** L226 (AgentPhase: Idle | Running), L358 (HarnessPhase: Idle | Turning | Compacting)

两套阶段机并存，交互不明确：

| 场景 | Agent 状态 | Harness 状态 | 行为？ |
|---|---|---|---|
| 用户调用 `harness.prompt()` | ? | Turning | 启动 agent_loop |
| 用户调用 `harness.compact()` | ? | Compacting | compaction 调用 LLM 做摘要 |
| compaction 期间收到 `prompt()` | ? | Compacting | 应返回 `Err(NotIdle)` (L372-373) |

**核心问题：** `compact()` 需要调用 LLM（`summary_model`）。这个 LLM 调用是否复用 Agent？如果复用，Agent 必须处于 Idle；如果独立调用，为什么要把 `compact()` 放在 Harness 层且与 `prompt()` 互斥？

**建议：** 明确 compaction 的 LLM 调用独立于 Agent。考虑允许 Agent 在 Idle 时异步执行 compaction（后台压缩），使 compaction 延迟对用户不可见。

---

### 7. `should_stop` hook 与 LLM 自然停止的优先级

**涉及行：** L175

两种停止信号可能冲突：

| LLM 返回 | should_stop 返回 | 期望行为？ |
|---|---|---|
| stop_reason: EndTurn | false（调用方想继续） | 覆盖 LLM，继续下一轮？ |
| tool_use（LLM 想继续） | true（调用方想停） | 覆盖 LLM，停止 loop？ |

优先级不明确。此外，`should_stop` 是 `Arc<dyn ShouldStopHook>` —— trait 的签名未知，它是同步判断还是一步判断？如果 hook 需要查询外部状态（如用户点击了"停止"按钮），它可能需要是 async 的。

---

### 8. HarnessHooks 与 LoopConfig hooks 的重复

**涉及行：** L167-168 (LoopConfig 的 before/after_tool_call), L363-364 (HarnessHooks 的 before/after_tool_call)

两层都有 `BeforeToolCallHook` 和 `AfterToolCallHook`。它们是相同的 trait 还是不同的？如果相同，Harness 层的是覆盖 Loop 层的还是追加？如果不同，各自的职责边界是什么？

**建议：** 要么只保留一层（推荐 Harness 层），Loop 层只保留 `transform_context` 和 channel；要么明确两层 hook 的执行顺序（哪层先执行，是否短路）。

---

## 🟢 次要问题与改进建议

### 9. `Tool::execute` 的生命周期约束限制并发模型

**涉及行：** L109-113

```rust
fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a ToolContext)
    -> BoxFuture<'a, Result<ToolResult, ToolError>>;
```

返回的 `BoxFuture` 借用了 `&self` 和 `&ToolContext`，不满足 `'static`。这意味着：
- 不能将 tool 执行 `spawn` 到独立 tokio task
- Tool 实例在执行期间不能被释放（这本身合理）
- 但对于需要长时间运行的 tool（shell、网络请求），限制了通过 `tokio::spawn` 实现真正的并行

**建议：** 评估是否允许 tool 返回 `'static` future，要求 tool 内部状态通过 `Arc` 共享。或者至少文档化这个约束的设计意图（安全优先、避免 spawn 开销等）。

---

### 10. broadcast channel 事件静默丢失

**涉及行：** L219 `event_tx: broadcast::Sender<AgentEvent>`

`tokio::sync::broadcast` 有界通道在接收端消费慢时会静默丢弃旧事件。`TurnStart`/`TurnEnd` 是状态边界事件，丢失后订阅者的状态机可能失配（以为还在上一轮）。`ToolCallStart` 丢失而 `ToolCallEnd` 到达会导致 ID 匹配失败。

**建议：**
- 至少文档化"慢消费者将丢失事件"
- 考虑为 `TurnStart`/`TurnEnd` 等状态边界事件使用 `watch` channel（只保留最新值，不会丢失状态）
- 或为需要完整事件的订阅者提供 `UnboundedReceiver` 替代方案

---

### 11. Steer 消息的语义选择 —— 队列式 vs 覆盖式

**涉及行：** L197-198

通过 `mpsc::Receiver` 接收 steer 消息。如果在一个 turn 期间连续调用 `steer("A")`、`steer("B")`：
- **队列式（当前行为）：** 两者都在 channel 中排队，依次注入
- **覆盖式：** 只取最新一条，"B" 覆盖 "A"

哪种是期望行为？steer 的设计意图是"影响当前轮次的走向"，多条 steer 是否需要全部注入？如果 steer 是响应式纠正（"停，换个方向"），覆盖式更合适。

---

### 12. `ContentBlock::Image` 缺少 URL/引用变体

**涉及行：** L60

```rust
Image { media_type: String, data: String }  // base64
```

仅支持 base64 内联。问题：
- base64 膨胀 33%，影响传输效率
- 无法利用多模态 API 的图片缓存（如 Anthropic 的 image content block 支持 `source` 引用）
- 重复引用同一图片时需要多次编码/传输

**建议：** 预留扩展点：

```rust
pub enum ImageSource {
    Base64 { media_type: String, data: String },
    // 预留:
    // Url { url: String },
    // Id { id: String },
}

pub struct ImageBlock {
    pub source: ImageSource,
}
```

---

### 13. 并发 prompt 调用未定义行为

**涉及行：** L237-238

```rust
// 结构性操作：仅 Idle 阶段可调用，否则返回 Err(NotIdle)
pub async fn prompt(&self, text: impl Into<String>) -> Result<()>;
```

当 Agent 处于 `Running` 时，另一个调用方调用 `prompt()` 会得到 `Err(NotIdle)`。但：
- 调用方需要自己实现重试/排队逻辑
- 没有内置的排队机制（如 "加入队列，Idle 后自动发送"）
- 如果两个调用方**同时**在 `Idle` 状态下调用 `prompt()`，只有一个会成功改变状态为 `Running`，另一个的行为取决于锁的实现（返回错误 vs panic）

**建议：** 至少文档化这个行为，并在 `Agent` 或 `AgentHarness` 层提供可选的排队机制。

---

### 14. `ExecutionEnv` 缺少权限模型

**涉及行：** L129-136

`ExecutionEnv` trait 的方法（`read_file`, `write_file`, `execute_shell`）没有任何权限控制。TS 版可能依赖 Node.js 的进程权限，但 Rust 重写需要自己决定权限边界。

- 权限由 `ExecutionEnv` 实现方负责吗？
- `Tool` 通过 `ToolContext.env` 获得 `ExecutionEnv`——tool 可以读写任意文件、执行任意命令
- 是否需要 `ExecutionEnv` 提供某种能力令牌（capability token）机制？

**建议：** 至少在 trait 文档中明确权限模型的职责归属，并在 `ToolContext` 中预留权限范围字段。

---

### 15. Session log 对超大 entry 的处理

**涉及行：** L270-279

`SessionEntry::Message(MessageEntry)` 可能包含大型工具结果（如 shell 命令完整输出）。JSONL 格式下每条 entry 一行，超大 entry 会导致：
- 单行解析性能退化
- 内存峰值
- 分支切换时需要重放大量数据

**建议：** 考虑为大型 entry 提供外部存储引用（如 `LargeContent { id: EntryId, storage_key: String }`），或至少定义单条 entry 的大小上限。

---

## 审核总结

| 类别 | 数量 | 阻塞发布？ | 关键项 |
|---|---|---|---|
| 🔴 严重 | 3 | 是 | Mutex 死锁、缺失类型定义、tool batch 降级 |
| 🟡 重要 | 5 | 建议修复 | 分支模型空洞、compaction 交互、阶段交互、stop 优先级、hook 重复 |
| 🟢 次要 | 7 | 否 | 生命周期约束、事件丢失、steer 语义、图片 URL、并发控制、权限模型、超大 entry |

**建议：** 🔴 项必须在进入实现阶段前解决；🟡 项应在实现前讨论定案，至少给出设计决策记录；🟢 项可在实现中迭代，但需在文档中标注为已知权衡。
