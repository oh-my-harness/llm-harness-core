# llm-harness-core 与 pi-agent-core 对齐审核

**日期：** 2026-06-07
**审核对象：** [2026-06-07-llm-harness-core-design.md](./2026-06-07-llm-harness-core-design.md)
**对比基准：** `/Users/hhl/Documents/projs/pi-main/packages/agent/src/`
**状态：** 待讨论

---

## 审核方法

逐模块对比 TS 实现与 Rust 设计，标注偏离程度：

| 标记 | 含义 |
|---|---|
| ✅ | 对齐，Rust 惯用表达等价 |
| ⚠️ | 简化/偏离，需确认是否 intentional |
| 🔴 | 重大缺失，影响功能对标 |
| ➕ | TS 中没有，Rust 新增（需确认价值） |

---

## 1. 消息与类型系统

### 1.1 ContentBlock / AgentMessage 模型

| TS (`types.ts`) | Rust 设计 | 判定 |
|---|---|---|
| `AgentMessage = Message \| CustomAgentMessages[keyof CustomAgentMessages]` — 可扩展联合类型 | `AgentMessage` 固定 enum：User/Assistant/ToolResult/Custom | ⚠️ |
| CustomAgentMessages 通过 declaration merging 扩展：`bashExecution`, `custom`, `branchSummary`, `compactionSummary` | `CustomMessage { r#type: String, data: Value }` — 单一 custom 变体 | 🔴 |
| `AgentMessage` 直接复用 `@earendil-works/pi-ai` 的 `Message` 类型（含 `timestamp`, `api`, `provider`, `model`, `usage`, `stopReason`, `errorMessage`） | `UserMessage`/`AssistantMessage`/`ToolResultMessage` 仅含 `content` + 最少字段 | 🔴 |

**关键偏离：**
- TS 的 `AgentMessage` 是 **富类型**——AssistantMessage 携带 `usage`（token 统计）、`stopReason`、`errorMessage`、`api`、`provider`、`model`、`timestamp`。这些是 compaction token 估算、错误处理、session 回放的关键数据。
- Rust 设计的 `AssistantMessage` 仅有 `content` 和 `stop_reason: Option<StopReason>`，缺失整个 usage/error 维度。
- TS 的 declaration merging 模式允许应用层扩展消息类型（`bashExecution`, `branchSummary`, `compactionSummary`）。Rust 用 `CustomMessage` 替代，丢失了类型安全。

### 1.2 ContentBlock 粒度

| TS | Rust | 判定 |
|---|---|---|
| `TextContent { type, text, textSignature? }` | `Text { text: String }` | ⚠️ |
| `ImageContent { type, media_type, data?, url? }` | `Image { media_type, data }` — 仅 base64 | ⚠️ |
| `ToolCall { type, id, name, arguments }` | `ToolUse { id, name, input }` | ✅ |
| `ThinkingContent { type, thinking, thinkingSignature? }` | 无 | 🔴 |

**关键偏离：** TS 的 `ThinkingContent` 是 Anthropic extended thinking 的核心载体。Rust 设计完全没有 thinking 内容块，这意味着无法在消息历史中保留 thinking 痕迹（对 compaction 和上下文重建很重要）。

### 1.3 EntryId 设计

| TS | Rust | 判定 |
|---|---|---|
| `string`（UUIDv7，含时间戳信息） | `[u8; 16]`（UUIDv7） | ⚠️ |

TS 使用字符串 ID 便于 JSONL 存储和跨系统引用。Rust 用 `[u8; 16]` 更紧凑，但与外部系统的互操作需要序列化。这是合理的设计选择，但需要关注 session fork 时 ID 的字符串表示。

---

## 2. Agent Loop 引擎

### 2.1 事件模型 —— 最大的偏离

**TS 事件** (`types.ts` L403-418)：

```
agent_start → turn_start → message_start → message_update* → message_end
  → [tool_execution_start → tool_execution_update* → tool_execution_end]+
  → turn_end { message, toolResults }
  → ... (更多 turn)
  → agent_end { messages }
```

**Rust 事件** (设计 L89-98)：

```
TurnStart { index } → TextDelta* | ToolCallStart → ToolCallDelta* → ToolCallEnd { result }
  → TurnEnd { index } → Done
```

| TS 事件 | Rust 对应 | 判定 |
|---|---|---|
| `agent_start` | 无 | 🔴 |
| `agent_end { messages }` | `Done`（无 payload） | 🔴 |
| `turn_start` | `TurnStart { index }` | ⚠️ |
| `turn_end { message, toolResults }` | `TurnEnd { index }`（无 payload） | 🔴 |
| `message_start { message }` | 无 | 🔴 |
| `message_update { message, assistantMessageEvent }` | `TextDelta { text }` + `ToolCallDelta` | 🔴 |
| `message_end { message }` | 无 | 🔴 |
| `tool_execution_start { toolCallId, toolName, args }` | `ToolCallStart { id, name }` | ⚠️ 缺 args |
| `tool_execution_update { partialResult }` | 无 | 🔴 |
| `tool_execution_end { result, isError }` | `ToolCallEnd { result }` | ⚠️ 缺 isError |

**这是整个设计中最重大的偏离。** TS 的事件模型是 **消息级** 的（message_start/update/end），Rust 设计是 **token 级** 的（TextDelta）。两者服务于不同的 UI 范式：

- TS 事件模型驱动的是消息列表 UI，每条消息有明确的开始/更新/结束生命周期
- Rust 的 TextDelta 模型适合字符级流式渲染，但不表达消息边界

此外，`agent_start`/`agent_end` 的缺失意味着调用方无法感知 agent 生命周期的开始和结束。`agent_end` 携带 `messages: AgentMessage[]`（本轮新增的所有消息），这是 Agent 和 AgentHarness 之间传递结果的关键通道。

**建议：** 在 Stream 之外增加消息级事件包装层，或重新设计事件模型至少覆盖 `agent_start`/`agent_end`/`message_start`/`message_end`/`turn_end` 的 payload。

### 2.2 convertToLlm —— 缺失的关键抽象

**TS** (`agent-loop.ts` L283-289)：每轮 LLM 调用前执行两步变换：
1. `transformContext(messages, signal)` → AgentMessage[] （可选，用于 compaction）
2. `convertToLlm(messages)` → Message[] （**必须**，AgentMessage → LLM Message）

`convertToLlm` 是必需的，因为：
- `BashExecutionMessage` → 转为 `UserMessage`（格式化命令+输出）
- `CustomMessage` → 转为 `UserMessage`
- `BranchSummaryMessage` → 转为带特殊前缀的 `UserMessage`
- `CompactionSummaryMessage` → 转为带特殊前缀的 `UserMessage`

**Rust 设计：** 没有 `convert_to_llm`。`LoopConfig` 只有 `transform_context`。设计隐含假设 AgentMessage 可以直接用作 LLM 的输入，但 `CustomMessage` 等非标准消息类型无法被 LLM 理解。

**判定：** 🔴 重大缺失。必须在 loop 和 LLM 调用之间插入转换层。

### 2.3 QueueMode

**TS** (`types.ts` L44)：`QueueMode = "all" | "one-at-a-time"` 控制 steer/followUp 队列在每次排空点取出多少条消息。

**Rust 设计：** 无 QueueMode。mpsc channel 的 `try_recv` 每次只取一条，相当于硬编码 `"one-at-a-time"`。

**判定：** ⚠️ 简化。如果 TS 的 `"all"` 模式有实际使用场景，需要考虑是否支持。

### 2.4 shouldStopAfterTurn / terminate 语义

**TS：**
- `shouldStopAfterTurn(context)` — hook 返回 true 时 agent 停止
- Tool result 的 `terminate` 标志 — 当一个 batch 中所有 tool 都设置 `terminate: true` 时提前停止

**Rust 设计：**
- `should_stop: Option<Arc<dyn ShouldStopHook>>` — 类似
- 无 `terminate` 语义

**判定：** ⚠️ `terminate` 是重要的工具驱动停止机制（如 tool 判断任务完成），缺失会影响自主 agent 的停止能力。

### 2.5 prepareNextTurn

**TS** (`types.ts` L214-217)：`prepareNextTurn` hook 在每个 turn 结束后调用，可返回新的 context/model/thinkingLevel，实现动态配置更新。

**Rust 设计：** 无此 hook。Turn snapshot 机制只保证 turn 内一致性，不支持 turn 间的动态配置变更。

**判定：** ⚠️ 缺失。在 AgentHarness 中通过 `prepareNextTurn` 实现每轮重建上下文（含从 session log 重建），这是 Harness 与 Loop 的关键集成点。

### 2.6 tool_execution_update（流式工具输出）

**TS** (`types.ts` L357-358)：`AgentToolUpdateCallback` — 工具执行期间可以流式推送部分结果，UI 可以渐进渲染。

**Rust 设计：** `Tool::execute` 返回 `BoxFuture<Result<ToolResult, ToolError>>`，无流式回调。事件模型中无 `ToolCallDelta` 用于中途更新（`ToolCallDelta` 目前仅用于 tool call **参数**的增量，而非执行的中间结果）。

**判定：** ⚠️ 对长时间运行的工具（shell、网络请求），缺少流式输出能力会影响用户体验。

---

## 3. Agent（有状态包装器）

### 3.1 AgentState 字段

| TS `AgentState` (`types.ts` L317-342) | Rust `AgentState` (设计 L224-232) | 判定 |
|---|---|---|
| `systemPrompt: string` | `system_prompt: Option<String>` | ✅ |
| `model: Model<any>` | `model: String` | ⚠️ |
| `thinkingLevel: ThinkingLevel` | `thinking_level: ThinkingLevel` | ✅ |
| `tools: AgentTool[]`（getter/setter 自动 copy） | `tools: Vec<Arc<dyn Tool>>` | ⚠️ |
| `messages: AgentMessage[]`（getter/setter 自动 copy） | `messages: Vec<AgentMessage>` | ✅ |
| `isStreaming: boolean`（只读） | `phase: AgentPhase`（Idle/Running） | ✅ |
| `streamingMessage?: AgentMessage`（只读） | 无 | 🔴 |
| `pendingToolCalls: ReadonlySet<string>`（只读） | 无 | 🔴 |
| `errorMessage?: string`（只读） | 无 | 🔴 |

**关键偏离：**
- **`streamingMessage`** — TS 在流式响应期间持续更新此字段，UI 可以渲染部分消息。缺失则调用方无法获取进行中的消息。
- **`pendingToolCalls`** — TS 允许调用方查询当前正在执行的工具。缺失则无法显示"正在执行 X..."的状态。
- **`errorMessage`** — TS 保留最近一次失败的 LLM 响应的错误信息。缺失则调用方需要自己追踪错误。
- **`model` 类型** — TS 用完整的 `Model<any>` 对象（含 provider, api, contextWindow, maxTokens, cost 等元数据），Rust 仅用 `String`。Compaction token 估算依赖 model 的 `contextWindow` 和 `maxTokens`。

### 3.2 Agent 方法

| TS `Agent` 方法 | Rust `Agent` 方法 | 判定 |
|---|---|---|
| `prompt(text)` / `prompt(message[])` | `prompt(text)` | ⚠️ 不支持消息数组 |
| `continue()` | 无 | 🔴 |
| `steer(message)` | `steer(text)` — 仅 String | ⚠️ |
| `followUp(message)` | `follow_up(text)` — 仅 String | ⚠️ |
| `clearSteeringQueue()` | 无 | 🔴 |
| `clearFollowUpQueue()` | 无 | 🔴 |
| `clearAllQueues()` | 无 | 🔴 |
| `hasQueuedMessages()` | 无 | 🔴 |
| `reset()` | 无 | 🔴 |
| `waitForIdle()` | `wait_for_idle()` | ✅ |
| `subscribe(listener) → unsubscribe` | `subscribe() → Receiver` | ✅ |
| `signal` (getter) | `abort: CancellationToken` | ✅ |
| `abort()` | `abort()` | ✅ |

**关键偏离：**
- **`continue()`** — TS 支持从当前 transcript 继续执行（无需新 prompt）。AgentHarness 的 `prepareNextTurn` 依赖此机制（每 turn 后重建上下文继续执行）。Rust 设计缺失此能力。
- **队列清空方法** — 缺 `clearSteeringQueue`/`clearFollowUpQueue`/`clearAllQueues`/`hasQueuedMessages`。这些是 abort 流程中的关键操作。
- **`reset()`** — 清空 transcript 和运行时状态。对长生命周期 agent 复用很重要。

---

## 4. Session —— 最重大的架构偏离

### 4.1 数据模型：Tree vs Log

**TS Session 是树结构：**

```typescript
// 每个 entry 有 parentId，形成树
interface SessionTreeEntryBase {
    type: string;
    id: string;         // UUIDv7 字符串
    parentId: string | null;  // 父节点 ID，null = root
    timestamp: string;
}
```

**Rust 设计是线性日志：**

```rust
pub trait SessionStorage {
    fn append(&self, entry: SessionEntry) -> BoxFuture<Result<EntryId>>;
    fn read_range(&self, from: Option<EntryId>) -> BoxFuture<Result<Vec<(EntryId, SessionEntry)>>>;
}
```

| 维度 | TS | Rust | 判定 |
|---|---|---|---|
| 数据结构 | 树（parentId 链接） | 线性追加日志 | 🔴 |
| 分支支持 | 完整（fork, moveTo, leaf tracking） | 声明支持但无设计 | 🔴 |
| 叶子追踪 | `getLeafId()` / `setLeafId()` | 无 | 🔴 |
| 路径查询 | `getPathToRoot(leafId)` | 无（只有 `read_range`） | 🔴 |
| Fork | `SessionRepo.fork(source, options)` | 无 | 🔴 |
| 上下文重建 | `buildContext()` — 从 root 到 leaf 重放，应用 compaction 过滤 | 无 | 🔴 |

**这是 Rust 设计与 TS 实现之间最根本的架构偏离。** TS 的 session 树是支撑以下功能的基础：
- 分支对话（用户回到历史点重新开始）
- 分支导航（`navigateTree`）
- 分支摘要（`BranchSummary`）
- Compaction 的正确实现（需要在树路径上定位 `firstKeptEntryId`）

Rust 设计的线性日志模型无法表达分支。如果 v1 不做分支，至少要：
1. 在 `SessionEntry` 中保留 `parent_id: Option<EntryId>` 字段
2. 在 `SessionStorage` 中保留 `get_leaf_id` / `set_leaf_id` / `get_path_to_root` 方法
3. 否则后续添加分支将是破坏性变更

### 4.2 Session Entry 类型

| TS Entry 类型 | Rust Entry 类型 | 判定 |
|---|---|---|
| `message` | `Message(MessageEntry)` | ✅ |
| `thinking_level_change` | `ThinkingLevelChange { to }` | ✅ |
| `model_change` | `ModelChange { to }` | ⚠️ TS 含 provider+modelId |
| `active_tools_change` | `ToolsChange { active }` | ✅ |
| `compaction` | `Compaction(CompactionEntry)` | ⚠️ 详见 5.2 |
| `branch_summary` | `BranchSummary(BranchSummaryEntry)` | ✅ 存在但未展开 |
| `custom` | 无 | 🔴 |
| `custom_message` | 无 (通过 AgentMessage::Custom 替代) | ⚠️ |
| `label` | `Label { name }` | ✅ |
| `session_info` | 无 | 🔴 |
| `leaf` | `Leaf` | ⚠️ 无 targetId |

**缺失项：**
- **`custom` entry** — 应用层向 session log 追加自定义结构化数据的能力（如 "用户打开了文件 X"）
- **`session_info` entry** — 会话命名（`appendSessionName`），用于 UI 显示

### 4.3 SessionStorage trait

| TS `SessionStorage` 方法 | Rust `SessionStorage` 方法 | 判定 |
|---|---|---|
| `getMetadata()` | 无 | 🔴 |
| `getLeafId()` / `setLeafId()` | 无 | 🔴 |
| `createEntryId()` | 无（由调用方提供 EntryId） | ⚠️ |
| `appendEntry(entry)` | `append(entry)` | ✅ |
| `getEntry(id)` | 无（只有 `read_range`） | 🔴 |
| `findEntries(type)` | 无 | 🔴 |
| `getLabel(id)` | 无 | 🔴 |
| `getPathToRoot(leafId)` | 无 | 🔴 |
| `getEntries()` | `read_range(from)` — 语义不同 | ⚠️ |

`read_range(from: Option<EntryId>)` 语义不清晰：
- `from = None` 表示从头开始还是从最新开始？
- 返回的是线性范围还是树路径？
- 如果 session 有 10 万条 entry，全量读取不可行

### 4.4 SessionRepo（持久化仓库）

**TS：** `SessionRepo` 提供 CRUD + fork：
- `create(options)` → Session
- `open(metadata)` → Session
- `list(options)` → Metadata[]
- `delete(metadata)`
- `fork(source, options)` → Session

**Rust 设计：** 仅有 `JsonlSessionStorage` 和 `InMemorySessionStorage` 两个具体实现，没有仓库抽象层。

**判定：** 🔴 缺失。SessionRepo 是管理多个 session 生命周期的关键抽象。没有它，调用方需要自己管理文件路径和元数据。

---

## 5. Compaction

### 5.1 复杂度差距

**TS compaction 管道：**

```
prepareCompaction(pathEntries, settings)
  → 找到上次 compaction 边界
  → 估算 token 数
  → 找到 cut point（考虑 keepRecentTokens）
  → 处理 split-turn（跨 turn 边界的 cut）
  → 提取 file operations
  → 返回 CompactionPreparation

compact(preparation, model, apiKey, ...)
  → generateSummary (或 updateSummary 如有 previousSummary)
  → 处理 split-turn prefix 独立摘要
  → 附加 file operations 列表
  → 返回 CompactionResult
```

**Rust 设计：**

```rust
pub async fn compact(
    client: &dyn LlmClient,
    messages: &[AgentMessage],
    settings: &CompactionSettings,
) -> Result<CompactionResult>
```

| TS 特性 | Rust 设计 | 判定 |
|---|---|---|
| 基于 session tree entries 操作（非纯 messages） | 基于 `&[AgentMessage]` | 🔴 |
| 迭代摘要更新（previousSummary） | 无 | 🔴 |
| Split-turn 处理（turn prefix 独立摘要） | 无 | 🔴 |
| 文件操作追踪（跨 compaction 累积） | 无 | 🔴 |
| `CompactionPreparation` 中间类型 | 无（直接调用 compact） | ⚠️ |
| `CompactionSettings.enabled` 开关 | 无 | ⚠️ |
| 分开 `reserveTokens` / `keepRecentTokens` | 合并为 `token_threshold` + `retain_recent` | ⚠️ |
| `findCutPoint` — 精确到 entry 的截断点查找 | 无 | 🔴 |

**最重要的偏离：** TS compaction 操作在 **Session tree entries** 上（而非纯 messages），因为：
1. 需要在 tree 中定位上次 compaction 的 `firstKeptEntryId`
2. Cut point 需要考虑 entry 类型（不能在 toolResult 中间截断）
3. 需要保留 compaction 后的 entry 作为新的 `firstKeptEntryId`

Rust 设计将 compaction 视为纯 message 数组操作，与 session log 解耦——但 session log 才是 compaction 持久化的载体。这导致 compaction 结果无法正确写入 session。

### 5.2 CompactionEntry 字段

| TS | Rust | 判定 |
|---|---|---|
| `summary: string` | 含在 CompactionResult 中 | ✅ |
| `firstKeptEntryId: string` | 无（`compressed_ids` 语义不同） | 🔴 |
| `tokensBefore: number` | 无 | 🔴 |
| `details?: T`（含 readFiles, modifiedFiles） | 无 | 🔴 |
| `fromHook?: boolean` | 无 | ⚠️ |

`firstKeptEntryId` 是 compaction 正确性的关键——它标记了"从这个 entry 开始的历史仍然有效"，下一次 compaction 从它开始计算边界。`compressed_ids: Vec<MessageId>` 无法替代这个语义。

---

## 6. AgentHarness

### 6.1 与 Agent 的关系

**TS：** AgentHarness **包含** Agent 的所有功能，并增加 Session、Skills、Templates、StreamOptions、Auth、Hooks、Compaction、BranchSummary。

AgentHarness **不使用** Agent 类——它直接调用 `runAgentLoop()` 函数，并自己实现事件处理、状态管理、session 写入。

**Rust 设计：** AgentHarness **组合** Agent（`agent: Agent`），但同时又复制了很多 Agent 的概念（phase、hooks、subscribe）。AgentHarness 与 Agent 之间的职责边界模糊。

**判定：** ⚠️ 架构层面的偏离。TS 的 AgentHarness 是 Agent 的超集替代，不是包装器。

### 6.2 缺失的 AgentHarness 功能

| TS AgentHarness 功能 | Rust 设计 | 判定 |
|---|---|---|
| `getApiKeyAndHeaders` — 动态 API key 解析 | 无 | 🔴 |
| `streamOptions` — 传输层配置（timeout, retry, headers, metadata, cache） | 无 | 🔴 |
| `on(type, handler)` — 类型化 hook 注册 | 简化版 `HarnessHooks` | ⚠️ |
| `setModel` / `setThinkingLevel` / `setTools` / `setActiveTools` — 运行时配置变更 + session 记录 | 无 | 🔴 |
| `setResources` / `getResources` | 无（skills 在构造时加载） | ⚠️ |
| `skill(name)` — 按名称调用 skill | 无 | 🔴 |
| `nextTurn(text)` — 下轮注入队列 | 无 | 🔴 |
| `navigateTree(targetId)` — 分支导航 | 无 | 🔴 |
| `appendMessage` — 直接向 session 追加消息 | 无 | 🔴 |
| `promptFromTemplate` | `prompt_from_template` | ✅ |
| `pendingSessionWrites` — 运行中延迟写入 | 无 | 🔴 |

**关键偏离：**
- **`getApiKeyAndHeaders`** — TS 支持为每次 LLM 调用动态解析 API key（OAuth token 可能过期）。Rust 设计假设 `LlmClient` 已配置好认证，这在长运行 agent 场景下不够灵活。
- **运行时配置变更** — TS 允许在 agent 运行期间修改 model/thinkingLevel/tools，修改自动记录到 session log（通过 pending writes）。Rust 的 turn snapshot 机制保护了 turn 内一致性，但缺少 turn 间的配置变更入口。

### 6.3 Harness 事件系统

**TS AgentHarness 事件** (`types.ts` L634-660) 除了透传 `AgentEvent`，还有 20+ 自身的 hook 事件：

| TS 事件 | 用途 | 判定 |
|---|---|---|
| `before_agent_start` | 修改初始 prompt/systemPrompt/messages | 🔴 |
| `context` | 修改发送给 LLM 的上下文（transformContext） | ⚠️ loop 层有 |
| `before_provider_request` | 修改 stream options（timeout, retry, headers） | 🔴 |
| `before_provider_payload` | 修改请求 payload | 🔴 |
| `after_provider_response` | 观察响应 headers/status | 🔴 |
| `tool_call` | 拦截/阻止 tool 调用 | ⚠️ loop 层有 |
| `tool_result` | 修改 tool 结果 | ⚠️ loop 层有 |
| `session_before_compact` | 取消/自定义 compaction | 🔴 |
| `session_compact` | compaction 完成通知 | 🔴 |
| `session_before_tree` | 取消/自定义 branch summary | 🔴 |
| `session_tree` | 分支导航完成通知 | 🔴 |
| `model_update` / `thinking_level_update` / `tools_update` | 配置变更通知 | 🔴 |
| `resources_update` | skills/templates 变更通知 | 🔴 |
| `queue_update` / `save_point` / `abort` / `settled` | 队列和生命周期通知 | 🔴 |

**判定：** Rust 设计的 `HarnessHooks` (L360-365) 仅有 4 个 hook（before_turn, after_turn, before_tool_call, after_tool_call），而 TS 有 20+ 个事件。这不是 Rust vs TS 的惯用差异——这是可扩展性上的实质差距。TS 的 hook 系统允许应用层深度定制 agent 行为，Rust 设计目前不具备此能力。

---

## 7. Skills 与 PromptTemplates

### 7.1 Skills 加载

| TS | Rust | 判定 |
|---|---|---|
| 递归目录遍历 | ✅ 提到 | ✅ |
| `.gitignore` / `.ignore` / `.fdignore` | 仅 `.gitignore` | ⚠️ |
| 每目录只取第一个 SKILL.md（不递归子目录的 SKILL.md） | 描述为"递归扫描" | ⚠️ |
| 名称校验：小写字母+数字+连字符，长度≤64 | 无 | 🔴 |
| 描述校验：必填，长度≤1024 | 无 | 🔴 |
| `disableModelInvocation` 支持 | 无 | 🔴 |
| name 必须匹配父目录名 | 无 | ⚠️ |
| 符号链接解析 | 无 | ⚠️ |
| `loadSourcedSkills`（带 source provenance） | 无 | ⚠️ |
| `formatSkillInvocation(skill, additionalInstructions)` | `format_skill_for_system_prompt` — 不同用途 | ⚠️ |

**关键偏离：**
- TS 的 skill 有 **两种使用方式**：(1) 注入 system prompt 供 LLM 自主选择调用；(2) 显式调用 `harness.skill(name)` 将 skill 内容作为 `<skill>` 块注入。Rust 设计只有前者。
- `disableModelInvocation` 允许 skill 对 LLM 不可见但仍可显式调用——这是重要的安全机制。

### 7.2 PromptTemplates

| TS | Rust | 判定 |
|---|---|---|
| 参数占位符：`$1`, `$@`, `$ARGUMENTS`, `${@:N}`, `${@:N:L}` | `{{placeholder}}` | 🔴 |
| 参数解析：shell-style 引号解析 | 无 | 🔴 |
| `invoke_template(template, args)` 用法 | 同 | ✅ |
| arg 为 `string[]`（位置参数） | arg 为 `HashMap<String, String>`（命名参数） | ⚠️ |

**关键偏离：** TS 使用位置参数（`$1`, `$2`）+ shell 风格引号解析。Rust 设计用 `{{placeholder}}` 命名参数。这两种模型完全不兼容——现有 prompt template 文件需要全部重写。

---

## 8. 工具系统

### 8.1 AgentTool vs dyn Tool

| TS `AgentTool<TParameters, TDetails>` | Rust `dyn Tool` | 判定 |
|---|---|---|
| `name: string` | `name() -> &str` | ✅ |
| `label: string` | 无 | 🔴 |
| `description: string` | `description() -> &str` | ✅ |
| `parameters: TSchema` (typebox) | `parameters_schema() -> &Value` | ✅ |
| `prepareArguments?: (args) => Static<TParameters>` | 无 | 🔴 |
| `execute(toolCallId, params, signal, onUpdate)` | `execute(args, ctx)` | 🔴 |
| `executionMode?: ToolExecutionMode` (per-tool) | `execution_mode() -> ToolExecutionMode` (per-tool) | ✅ |
| 泛型 `<TParameters, TDetails>` | 无泛型 | ⚠️ |

**关键偏离：**
- **`label`** — TS 工具有人类可读标签，用于 UI。缺失意味着 UI 只能显示 `name`。
- **`prepareArguments`** — 在 schema 验证前对原始 LLM 参数做兼容转换。处理 LLM 参数格式演化的关键机制。
- **`execute` 签名** — TS 版本的 `execute` 接收 `toolCallId`（用于关联事件）、`signal`（独立 AbortSignal）、`onUpdate`（流式回调）。Rust 版本将这些合并到 `ToolContext` 中，但丢失了 `toolCallId` 和 `onUpdate`。
- **泛型** — TS 的 `AgentTool<TParameters, TDetails>` 在 `execute` 中提供类型安全的参数和返回值。Rust 的 `serde_json::Value` 失去编译期保证。

---

## 9. 执行环境

### 9.1 ExecutionEnv 方法

| TS `FileSystem` + `Shell` | Rust `ExecutionEnv` | 判定 |
|---|---|---|
| `cwd: string` | `working_dir() -> &Path` | ✅ |
| `absolutePath(path)` | 无 | 🔴 |
| `joinPath(parts)` | 无 | 🔴 |
| `readTextFile(path)` | `read_file(path)` | ⚠️ 无 abortSignal |
| `readTextLines(path, { maxLines })` | 无 | 🔴 |
| `readBinaryFile(path)` | 无 | 🔴 |
| `writeFile(path, content)` | `write_file(path, content)` | ✅ |
| `appendFile(path, content)` | 无 | 🔴 |
| `fileInfo(path)` | 无（list_dir 合并了部分） | 🔴 |
| `listDir(path)` | `list_dir(path)` | ✅ |
| `canonicalPath(path)` | 无 | 🔴 |
| `exists(path)` | 无 | 🔴 |
| `createDir(path, { recursive })` | 无 | 🔴 |
| `remove(path, { recursive, force })` | 无 | 🔴 |
| `createTempDir(prefix)` | 无 | 🔴 |
| `createTempFile(options)` | 无 | 🔴 |
| `exec(command, options)` | `execute_shell(cmd, abort)` | ⚠️ 缺 cwd/env/timeout/onStdout/onStderr |
| 所有方法接受 `abortSignal?: AbortSignal` | 仅 execute_shell 有 `CancellationToken` | 🔴 |
| `cleanup()` | 无 | 🔴 |
| 所有方法返回 `Result<T, Error>`（不抛异常） | 返回 `Result<T>` | ✅ |

**判定：** 🔴 Rust 的 `ExecutionEnv` trait 严重简化。TS 版本有 17 个方法 + 选项对象，Rust 有 5 个方法。缺失的方法（如 `exists`, `createDir`, `remove`, `appendFile`）是 agent 工具实现的常用操作。`abortSignal` 的普遍缺失使得文件操作无法被取消。

### 9.2 Shell 选项

**TS `ExecutionEnvExecOptions`：** `cwd`, `env`, `timeout`, `abortSignal`, `onStdout`, `onStderr`

**Rust `execute_shell`：** `cmd: &str, abort: CancellationToken`

缺失 `cwd` 覆盖、环境变量注入、超时控制、流式 stdout/stderr 回调。

---

## 10. 不在范围内的功能（已声明，不构成偏离）

以下功能 Rust 设计已在第 7 节声明为不在范围：

| 功能 | 说明 |
|---|---|
| Proxy 模式 | `streamProxy` — browser→backend 流式转发 |
| WASM 目标 | `ExecutionEnv` 已预留扩展点 |
| 规划/记忆管理 | Agent loop 以外的框架能力 |

此外以下 TS 功能在设计中也未体现，但未被声明排除：

| 功能 | TS 文件 | 说明 |
|---|---|---|
| `streamFn` 抽象 | `types.ts:24-26` | 可替换的 LLM 流函数 |
| `agentLoopContinue` | `agent-loop.ts:64-93` | 从现有上下文继续执行 |
| `EventStream` 包装 | `agent-loop.ts:145-150` | 带 completion predicate 的 stream |
| `onPayload` / `onResponse` 回调 | `types.ts` | provider 请求/响应拦截 |
| `thinkingBudgets` | Agent options | 按 thinking level 的 token 预算 |
| `maxRetryDelayMs` | Agent options | provider 请求重试延迟上限 |

---

## 汇总

### 按模块偏离程度

| 模块 | 对齐度 | 关键缺口 |
|---|---|---|
| 消息/类型系统 | 40% | ThinkingContent, usage/error 元数据, 可扩展消息类型 |
| Agent Loop | 35% | 消息级事件, convertToLlm, terminate, prepareNextTurn, QueueMode |
| Agent | 45% | streamingMessage, pendingToolCalls, continue(), reset() |
| **Session** | **15%** | **树结构, 分支, fork, leaf tracking, path query, SessionRepo** |
| Compaction | 20% | tree-based 操作, split-turn, 迭代摘要, 文件追踪 |
| AgentHarness | 20% | 动态 API key, 运行时配置变更, 20+ hook 事件, navigateTree |
| Skills | 50% | 名称校验, disableModelInvocation, 显式调用 |
| PromptTemplates | 30% | 位置参数 vs 命名参数, shell 引号解析 |
| 工具系统 | 40% | label, prepareArguments, onUpdate, 泛型 |
| ExecutionEnv | 25% | 12+ 缺失方法, abortSignal 传播, shell 选项 |

### 按优先级

**🔴 必须在 v1 实现前解决（否则无法对标 pi-agent-core）：**

1. **Session 树结构** — 这是整个 pi-agent-core 架构的基石。没有 parentId 树和分支支持，session fork、compaction 追踪、上下文重建都无法正确实现。
2. **消息级事件模型** — `agent_start`/`agent_end`/`message_start`/`message_end` 以及 `agent_end` 携带 `messages` payload 是 Agent ↔ AgentHarness 的接口契约。
3. **convertToLlm** — AgentMessage → LLM Message 转换是 loop 正确性的前提。
4. **AgentMessage 富类型** — `usage`, `stopReason`, `errorMessage` 是 compaction token 估算和错误处理的基础。
5. **Compaction 的 session entry 操作** — compaction 必须基于 session tree entries 而非纯 message 数组。

**⚠️ v1 应尽量对齐（功能缺口）：**

6. `continue()` / `agentLoopContinue` — 无此能力则 AgentHarness 无法实现 prepareNextTurn
7. 运行时配置变更（setModel/setTools/setActiveTools）+ session 记录
8. `getApiKeyAndHeaders` — 动态认证
9. `ExecutionEnv` 方法完整性 — 至少补 `exists`, `createDir`, `remove`, `fileInfo`
10. Tool 系统的 `label`, `onUpdate`, `terminate`

**➖ 可后续迭代：**
11. 完整的 AgentHarness hook 事件系统
12. PromptTemplate 位置参数兼容
13. Skill 显式调用、disableModelInvocation
14. Proxy 模式（已声明不在范围）
