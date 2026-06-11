# Code Review: `llm-harness-loop` crate

**日期**: 2026-06-11  
**审查方式**: 多维度并行审查（5 个专项 reviewer + 1 critic 对抗性验证）

## 审查范围
- **Spec**: `docs/superpowers/specs/2026-06-07-llm-harness-core-design-phase2-loop.md`
- **Tasks**: `docs/superpowers/plans/2026-06-07-phase2-loop.md`
- **审查文件**:
  - `crates/llm-harness-loop/src/loop_fn.rs`
  - `crates/llm-harness-loop/src/dispatch.rs`
  - `crates/llm-harness-loop/src/stream_state.rs`
  - `crates/llm-harness-loop/src/hooked_tool.rs`
  - `crates/llm-harness-loop/src/convert.rs`
  - `crates/llm-harness-loop/src/type_bridge.rs`
  - `crates/llm-harness-loop/src/config.rs`
  - `crates/llm-harness-loop/src/test_utils.rs`

## 总体结论: NEEDS_CHANGES

1 个 P0 问题，4 个 P1 问题需要修复后合入。

---

## P0（必须修复）

### P0-1: 未注册工具调用导致对话历史损坏并触发 API 400 错误

- **维度**: 健壮性
- **位置**: `loop_fn.rs:203-212, 215-221, 251-274`
- **问题**: LLM 调用未注册工具时，`filter_map` 将其从 `calls` 中过滤，但 `ToolExecutionStart` 已对该工具发出。`execute_tool_batch` 只对注册工具返回结果，未注册工具无 `ToolResultMessage` 加入 `ctx.messages`。下一轮 `chat_stream` 请求包含孤立的 `ToolUse` block 但无对应 `ToolResult`，Anthropic API 返回 400 终止 session。
- **证据**:
  ```rust
  // 为所有 tool_calls emit ToolExecutionStart（含未注册工具）
  for (id, name, args) in &tool_calls {
      yield AgentEvent::ToolExecutionStart { ... };
  }
  // 只对注册工具构造 ToolResultMessage
  for (id, result) in &results {  // results 来自 execute_tool_batch(calls, ...)
      yield AgentEvent::ToolExecutionEnd { ... };
      ctx.messages.push(tool_result_msg);  // 未注册工具不在此
  }
  ```
- **建议**: 在 `execute_tool_batch` 后，对每个在 `tool_calls` 中存在但不在 `results` 中的 `tool_use_id`，生成合成错误响应：emit `ToolExecutionEnd { result: Err(ToolError::Execution("tool not registered".into())) }` 并将对应的 `ToolResultMessage { is_error: true, content: [Text("Tool 'X' is not registered")], .. }` 推入 `ctx.messages`。

---

## P1（应该修复）

### P1-1: `before_provider_request` 和 `after_provider_response` hook 从未被调用

- **维度**: 需求/设计符合度
- **位置**: `loop_fn.rs:124-145`（LLM 调用段）
- **问题**: `LoopConfig` 中的 `before_provider_request` 和 `after_provider_response` 两个字段在 `run_loop` 全文无调用点。spec §4.8 明确描述调用时机：前者应在 `client.chat_stream()` 前修改 `StreamOptions`，后者应在流处理完成后用于观测（计费统计、延迟监控）。

  注：`auth` hook 属有意 placeholder——config.rs 注释和 tasks.md 均记录"目前不直接传递给 adapter，为将来保留"，不在此条范围。

- **证据**: `loop_fn.rs` LLM 调用段只有 retry 循环，`config.stream_options` 甚至未传入 `ChatRequest::builder`；两个 hook 字段从不被读取。
- **建议**: 在 `client.chat_stream(&req)` 调用前，若 `before_provider_request` 存在则调用并将修改后的 stream options 反映到请求中；在 `final_message` 拿到后（流处理完毕），若 `after_provider_response` 存在则调用并传入 usage 等元数据。

### P1-2: `ToolContext::update_tx` 接收端立即丢弃，工具进度推送通道完全失效

- **维度**: 健壮性/契约
- **位置**: `loop_fn.rs:235`
- **问题**: ctx_factory 闭包中 `let (tx, _rx) = mpsc::channel(16)`，`_rx` 在闭包返回后析构。任何工具调用 `ctx.update_tx.send(partial).await` 会立即得到 `SendError`（channel 已关闭）。`AgentEvent::ToolExecutionUpdate` 是死事件——loop 中没有任何代码读取 update channel 并转发事件。
- **证据**:
  ```rust
  move |tool_use_id| {
      let (tx, _rx) = mpsc::channel(16);  // _rx 在此行结束时析构
      ToolContext { update_tx: tx, ... }   // 发送端进入 ctx，接收端已消失
  }
  ```
- **建议**: 若短期内 `ToolExecutionUpdate` 不需要实现，应将 `update_tx` 文档化为"当前未连接，send 会静默失败"，并确保工具实现不依赖成功发送。完整实现需在循环中持有 rx 并通过外部 channel 转发事件。

### P1-3: `NextTurnDirective::thinking_level` 和 `active_tools` 被静默丢弃

- **维度**: 需求/设计符合度
- **位置**: `loop_fn.rs:289-291`（工具停止路径）和 `loop_fn.rs:358-360`（非工具停止路径）
- **问题**: `PrepareNextTurnHook` 返回的 `NextTurnDirective` 中，`thinking_level: Option<ThinkingLevel>` 和 `active_tools: Option<HashSet<String>>` 两个字段（在 `hooks.rs:41,45` 定义）在两处 directive 应用代码中均未被读取。期望通过 hook 动态调整推理深度或屏蔽危险工具的实现会静默失效。
- **证据**:
  ```rust
  // 两处 directive 应用代码仅处理三个字段：
  if let Some(new_ctx) = directive.context { ctx = new_ctx; }
  if let Some(m) = directive.model { config.model = m; }
  if let Some(tools) = directive.tools { config.tools = tools; }
  // directive.thinking_level 和 directive.active_tools 均未读取
  ```
- **建议**: 在两处 directive 应用代码中添加 `if let Some(tl) = directive.thinking_level { config.thinking_level = tl; }`。对 `active_tools` 需先在 LoopConfig 中增加对应字段或在 `tools_to_defs` 时进行过滤，否则应从 `NextTurnDirective` 中移除该字段直至实现完整。

### P1-4: `AfterToolCallHook` 接收到错误类型归一化副本，丢失原始 `ToolError` 变体

- **维度**: 契约
- **位置**: `hooked_tool.rs:70-74`
- **问题**: `AfterToolCallCtx.result` 传入的是 `result_for_hook`（将所有 `ToolError` 变体归一化为 `Execution(string)` 的克隆），而非原始 `result`。`ToolError` 有 `InvalidArguments`/`Aborted`/`Execution`/`Other` 四个变体，hook 无法区分"工具被取消"与"执行错误"，variant-aware 的 after hook 逻辑完全无法实现。
- **证据**:
  ```rust
  let result_for_hook = result
      .as_ref()
      .map(|r| r.clone())
      .map_err(|e| ToolError::Execution(e.to_string()));  // 归一化：Aborted → Execution
  // ...
  let after_ctx = AfterToolCallCtx { result: &result_for_hook, ... };  // hook 看到降级副本
  ```
- **建议**: 将 `after_ctx.result` 改为引用原始 `result`：`result: &result`。`AfterToolCallCtx.result` 类型已经是 `&Result<ToolResult, ToolError>`，无需改变类型签名。移除 `result_for_hook` 绑定。

---

## P2（建议改进）

### P2-1: JSON 参数解析失败静默返回 `null`

- **维度**: 健壮性
- **位置**: `stream_state.rs:113, 176`
- **问题**: `serde_json::from_str(args).unwrap_or(Value::Null)` 吞掉解析错误，工具以 `null` 参数被调用时可能产生意外行为，且无任何诊断输出。tasks.md 有意为之，但缺乏可观测性。
- **建议**: 至少在解析失败时 emit 诊断事件或 tracing warn，让问题可被发现。

### P2-2: `build_final_content()` 内部存在冗余去重扫描

- **维度**: 性能/代码质量
- **位置**: `stream_state.rs:159`
- **问题**: `if indexed.iter().any(|(i, _)| i == idx) { continue; }` 在实践中永远不会触发（`block_order` 中每个 index 只出现一次），是死代码。同时每次 TextDelta 都触发 `partial_message()` → `build_final_content()` 克隆全量累积文本。
- **建议**: 移除冗余去重检查；若 `MessageUpdate` 事件的消费者不多，考虑用 dirty 标志减少 `partial_message()` 的构建频率。

### P2-3: `AssistantMessage.provider` / `.api` 字段永远为 `None`

- **维度**: 需求/设计符合度
- **位置**: `stream_state.rs:128-129`
- **问题**: Spec 要求这两个字段从流式事件中填入，但当前 adapter 的 `StreamHandle` 不提供 provider/api 信息。tasks.md 已承认这是已知差距，但代码中无任何说明。
- **建议**: 在 `StreamingState` 相关字段或 `AssistantMessage` 构造处添加注释，说明"当前 adapter 不支持，保留为 None"，避免下游依赖这两个字段。

### P2-4: `ModelInfo` pub use 重导出缺失

- **维度**: 需求/设计符合度
- **位置**: `lib.rs:29-37`
- **问题**: Spec 要求 `llm-harness-loop` 重导出 `ModelInfo`，下游 `llm-harness` 若需该类型将被迫直接依赖 `llm_adapter`，违反依赖隔离原则。
- **建议**: 在 `lib.rs` 中添加 `pub use llm_adapter::types::ModelInfo;`（或对应路径）。

### P2-5: `ContentBlock::Image` 在 assistant 消息转换时静默丢弃

- **维度**: 契约
- **位置**: `type_bridge.rs:71`
- **问题**: `content_block_to_response` 对 `Image` 变体返回 `None`，多轮对话中 assistant 消息里的 image block 被无声地从历史中移除，无警告无诊断。
- **建议**: 添加 tracing warn 记录数据丢失；在函数 doc comment 中说明此限制。

### P2-6: `pub mod dispatch` / `pub mod stream_state` 可见性过宽

- **维度**: 工程规范
- **位置**: `lib.rs:13, 16`
- **问题**: 两个模块内部只有 `pub(crate)` 接口，但模块本身声明为 `pub`。
- **建议**: 改为 `pub(crate) mod dispatch;` 和 `pub(crate) mod stream_state;`，与 `mod type_bridge;` 保持一致。

### P2-7: `RetryConfig::new()` 缺少 `///` doc comment

- **维度**: 工程规范
- **位置**: `config.rs:23`
- **问题**: Struct 及字段均有 doc comment，唯独 pub 构造函数缺失，违反 CLAUDE.md 自查要求。

### P2-8: 其余编码改进

- `execute_tool_batch` 中 `Arc::new(calls)` 仅为规避 move 所有权限制，可直接按索引取字段（`dispatch.rs:54`）
- `tools_to_defs` 每轮重建，若工具列表稳定可考虑缓存（`loop_fn.rs:107`）
- after hook 路径对 `ToolResult` 无条件克隆，`Passthrough` 时为浪费；可改为仅在 `Patch` 分支克隆（`hooked_tool.rs:71-73`）
- `.map(|r| r.clone())` 应替换为 `.cloned()`（`hooked_tool.rs:72`，clippy lint）
- 工具名匹配用 `O(m)` 线性搜索，可在 `LoopConfig` 构建时预建 `HashMap`（`loop_fn.rs:206`）

---

## Follow-up Notes

- **`AgentStart.initial_messages` 始终为空**: `agent_loop`（非 continue）路径的 `AgentStart` 始终发出 `initial_messages: vec![]`，与 spec §4.2 的文字描述有偏差（spec 说应携带初始注入消息）。tasks.md 的实现模板也写 `vec![]`，当前无下游消费方，消息通过后续事件可观测。若未来有 audit log 消费 `AgentStart`，需补齐此逻辑。

- **`dispatch.rs` 中 `results.into_iter().map(|r| r.unwrap())`**: 有不变量保护不会 panic，但 `results.into_iter().flatten()` 是更清晰的写法（`Option<T>` → `T`），可顺带改掉。

- **`auth` hook 的未来实现**: `auth` 被有意声明为 placeholder，但当前无任何机制向 adapter 传递动态凭证。将来实现时需重新设计 adapter 接口，以支持 per-request auth header 注入。

- **`ToolResultPatch.is_error` 无法生效**: `apply_patch` 不处理 `patch.is_error`，因为 `ToolResult` 结构体没有此字段（`is_error` 属于 `ToolResultMessage`，由 loop 从 `result.is_ok()/is_err()` 决定）。这是有意设计（tasks.md 注释："ToolResult has no is_error; ignore"），但可以在 `ToolResultPatch` 的字段 doc 中说明此限制，避免使用方困惑。
