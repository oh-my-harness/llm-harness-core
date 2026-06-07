# 从框架到可用的 Coding Agent：完整路线图

> ⚠️ **初步分析，非最终设计**
>
> 本文是基于阅读 TS 参考实现 `pi/packages/coding-agent` 的初步分析，
> 目的是梳理从当前 llm-harness-core 框架到可用 coding agent 需要建设的所有模块。
> 文档中的行数估算和模块划分是粗略推演，不是经过验证的计划。
> 在开始实现每个阶段之前，需要重新评估范围和优先级。
> 当前内容仅供参考和讨论。

**日期：** 2026-06-07
**来源：** 基于 TypeScript 参考实现 `pi/packages/coding-agent` 的分析
**目标：** 梳理从当前 `llm-harness-core` 框架到可用的 coding agent 需要建设的所有模块

---

## 一、当前状态

### ✅ 已完成（~3500 行 Rust）

| 阶段 | Crate | 内容 |
|---|---|---|
| **Phase 1** | `llm-harness-types` | 全部 11 个模块：messages、content、events、tool、env、errors、hooks、compaction（stub）、identity、misc、resources（stub）|
| **Phase 2** | `llm-harness-loop` | `agent_loop`/`agent_loop_continue`、`LoopConfig`、`HookedTool`、工具分治调度、`type_bridge`、`DefaultConvertToLlm`、`test_utils`（`MockLlmClient`）|

### ❌ 未完成

**框架层**（spec 中有设计，但无实现）：
- Phase 3: Agent
- Phase 4: Session
- Phase 5: Compaction
- Phase 6: Skills / Templates
- Phase 7: AgentHarness

**应用层**（coding-agent，spec 中未覆盖）：
- 全部 15 个模块（下文第二部分详述）

---

## 二、框架层：你还剩什么没做

### Phase 3: Agent（~600 行估算）

- **结构体：** `Agent` + `AgentState`
- **核心方法：** `prompt()`、`prompt_with_messages()`、`continue_run()`、`reset()`
- **队列：** `steer()` / `follow_up()` + `clear_all_queues()` + `has_queued_messages()`
- **运行时配置：** `set_model()` / `set_thinking_level()` / `set_tools()` / `set_system_prompt()`
- **控制：** `abort()` + `wait_for_idle()`
- **观测：** `state()`（clone 快照）、`subscribe()`（broadcast receiver）
- **锁模型：** `Arc<Mutex<AgentState>>` + 快照模式，不跨 `.await`

> 设计理由见 spec `phase3-agent.md` 的 §5.1。核心决策：Agent 是"不需要 session/skills"的轻量入口。它自己维护 `messages` 数组（不持久化）。

### Phase 4: Session（~1200 行估算）

- **SessionEntry 树：** `{type, id, parentId, timestamp} + payload`，真正的多分支树
- **JsonlStorage：** 文件追加写，内存树缓存（增量更新）
- **SessionRepo trait：** `create` / `open` / `list` / `delete` / `fork`
- **Session 结构体：** `new()` / `append()` / `navigate()` / `fork()` / `build_context()`
- **分支操作：** `active_cursor`（非 `active_leaf`）、`list_branches`、`get_tree`
- **build_context：** 从 entries 重建 `(messages, model, thinking_level, tools)`
- **锁策略：** `Mutex` + 快照模式，`append` 自动填 id/parent_id/timestamp

> `CompactionSummary` / `BranchSummary` 是框架内置的具名消息 variant。session 的 `fork` 跨 session 复制路径（不是硬 link）。

### Phase 5: Compaction（~400 行估算）

- **prepare 阶段：** `prepare_compaction()` → 找出 cut point（首个可压缩的 message entry）
- **execute 阶段：** `compact()` → 用 LLM 生成摘要 + 返回 `CompactionResult`
- **`CompactionResult`：** `{summary, first_kept_entry_id, tokens_before}`
- **`CompactSettings`：** `reserve_tokens`、`keep_recent_tokens`（无 `token_threshold`）
- **Harness 回调：** `compact()` 不写 session——返回 `CompactionResult`。由 Harness 在 `Ok` 分支写入

### Phase 6: Skills / Templates（~400 行估算）

- **Skill：** `{name, description, filePath, content}` + 参数替换
- **PromptTemplate：** `{name, description, template}` + 位置参数 + shell 风格引号解析
- **加载函数：** `load_skills_from_dir(dir)` / `load_prompt_templates(dir)`
- **返回值：** `(成功列表, Vec<Diagnostic>)`——不因单个文件失败而整体失败

### Phase 7: AgentHarness（~1500 行估算）

- **结构体：** `AgentHarness` + `HarnessState`
- **HarnessHooks：** 11 个 hook trait 的集合——`BeforeRun`、`BeforeTurn`、`AfterTurn`、`BeforeToolCall`、`AfterToolCall`、`TransformContext`、`PrepareNextTurn`、`ShouldStop`、`BeforeProviderRequest`、`AfterProviderResponse`、`BeforeCompact`
- **AgentHarnessEvent：** `AgentHarnessEvent` enum（session 写入事件、compaction 事件、branch 事件等）
- **核心方法：** `prompt()`、`continue_run()`、`compact()`、`navigate()`、`steer()`、`follow_up()`
- **session 写入管线：** `pending_session_writes` → `flush_pending_writes`（turn 结束时批量落盘）
- **`next_turn` 缓冲：** `queued_next_turn`——下次 `prompt()` 时注入
- **工具子集控制：** `active_tools: Option<HashSet<String>>`
- **LoopConfig 构造：** 每次启动 loop 时从 `HarnessHooks` + `HarnessState` 动态构造临时 `LoopConfig`

> 核心架构决策：AgentHarness **不包装 Agent**。它直接驱动 `agent_loop()`，自己管理状态，从 session 读消息历史（`HarnessState` 无 `messages` 字段）。消除了 TS 版中 Agent 的 `messages` 和 Session 的 entries 之间的冗余同步问题。

---

## 三、应用层（Coding Agent）：还需要建设什么

### 总体架构

```
┌──────────────────────────────────────────────────────────────┐
│  CLI / TUI / RPC Mode        (终端入口层)                      │
│  参数解析、三种运行模式                                        │
├──────────────────────────────────────────────────────────────┤
│  AgentSession                 (核心编排)                       │
│  业务逻辑胶水：连接 AgentHarness + Tools + Config + Extensions  │
├────────────────────┬──────────────────┬──────────────────────┤
│ Setting/Config/Auth│ Resource Loader  │ Tool Registry         │
│ (settings.json)    │ (skills/prompts/ │ (Read/Bash/Edit/Write │
│ AuthStorage        │  extensions/     │  Grep/Find/Ls)       │
│ ModelRegistry      │  context files)  │  共 7 个工具          │
├────────────────────┴──────────────────┴──────────────────────┤
│  Extension System （插件运行时）                                │
│  事件钩子、slash 命令、自定义工具注册、UI 组件                    │
├──────────────────────────────────────────────────────────────┤
│  llm-harness-core (你的框架) —— Phase 3-7                     │
└──────────────────────────────────────────────────────────────┘
```

### 模块清单

以下按"构建顺序"排列（上层依赖下层），每个模块标注与框架的接口点、设计要点、以及可参考的 TS 源码位置。

---

#### 1. ⌨️ Coding Tools（7 个工具）

**依赖的框架接口：** `impl Tool` trait、`ToolContext`、`ToolResult`、`ExecutionEnv`

| 工具 | TS 源码 | 核心逻辑 | 关键设计点 |
|---|---|---|---|
| **Read** | `tools/read.ts` | 按行范围读文件，支持 offset/limit 截断 | `maxLines` / `maxBytes` 限制；图片自动 resize 嵌入消息 |
| **Bash** | `tools/bash.ts` | 执行 shell 命令，流式 stdout/stderr | `ShellOptions`（cwd/timeout/abort）、`commandPrefix`、权限确认、长输出截断 |
| **Edit** | `tools/edit.ts` | 精确文本替换（string replace，非 diff） | **文件变异队列**（串行化对同一文件的并发编辑） |
| **Write** | `tools/write.ts` | 写入新文件，覆盖确认 | 路径安全校验（不允许写到 session dir）、`file-mutation-queue` |
| **Grep** | `tools/grep.ts` | 文本搜索（封装 `rg`/`grep`） | 上下文行数、包含/排除模式、`.gitignore` 感知 |
| **Find** | `tools/find.ts` | 文件搜索（glob 风格） | 深度限制、排除规则、结果截断 |
| **Ls** | `tools/ls.ts` | 目录列表（树状或列表） | 文件大小格式化、权限显示、symlink 安全 |

每个工具需要定义**四类元数据**：
- `description()` — LLM tool definition 的描述
- `parameters_schema()` — JSON Schema
- `prompt_snippet`（在 ToolDefinition 中） — system prompt 的一行摘要（如 `- read: Read file contents with line range support`）
- `prompt_guidelines` — 工具的使用准则，追加到 system prompt

> 实现建议：把工具定义成 plugin trait，这样第三方也可以注册自定义工具。框架的 `dyn Tool` 已支持。

---

#### 2. 🛠️ Tool Registry（工具注册中心）

**TS 源码：** `core/tools/index.ts` + `agent-session.ts` 的 `_buildRuntime()` / `_refreshToolRegistry()`

- **两层存储：** `ToolDefinition`（元数据——name/description/schema/snippet/guidelines）和 `AgentTool`（执行实例——实现了 `Tool` trait）
- **工具过滤：** `allowedToolNames`（白名单）和 `excludedToolNames`（黑名单），支持 `noTools: "all" | "builtin"`
- **动态开关：** `setActiveToolsByName()` 改变当前活跃工具列表
- **system prompt 重建：** 每次工具集变更后重建（因为 tools 变了，guidelines 也要跟着变）

**与框架的接口：** AgentHarness 的 `active_tools: Option<HashSet<String>>` + AgentState 的 `tools: Vec<Arc<dyn Tool>>`

---

#### 3. 📜 System Prompt Builder

**TS 源码：** `core/system-prompt.ts`

构建 system prompt 的拼接规则：

```
身份定义 → "You are an expert coding assistant..."
可用工具列表（name: 一行摘要）
行为准则（基于已启用工具动态生成）
项目上下文（CLAUDE.md / AGENTS.md 等）
Skills 列表（/skill:name 调用说明）
当前日期 + 工作目录
```

**关键设计点：**
- 允许**完全替换**（`customPrompt`）或**追加**（`appendSystemPrompt`）
- `promptGuidelines` 按工具集动态生成（有 bash 无 grep → "Use bash for file operations..."
- Skills 只有当 `read` 工具可时才追加（因为 LLM 需要能读 skill 文件）

**与框架的接口：** 纯函数，不依赖框架。输出 `String` 设置到 `AgentHarness.state.system_prompt`。

---

#### 4. ⚙️ Settings / Config / Auth

**TS 源码：** `core/settings-manager.ts`、`core/auth-storage.ts`、`core/model-registry.ts`

**SettingsManager：**

配置项分层：
- **项目级**（`.pi/settings.json`）和**全局级**（`~/.pi/agent/settings.json`），项目级覆盖全局
- **信任机制：** `ProjectTrustStore`——项目被 trust 后才加载项目级配置和脚本

| 配置组 | 关键字段 |
|---|---|
| 模型 | `defaultProvider`, `defaultModel`, `defaultThinkingLevel` |
| Compaction | `enabled`, `reserveTokens`, `keepRecentTokens` |
| Retry | `enabled`, `maxRetries`, `baseDelayMs` |
| Shell | `shellPath`, `shellCommandPrefix` |
| 传输 | `transport`（HTTP/SSE auto） |
| 资源 | `extensions`, `skills`, `prompts`, `themes`, `packages` |
| UI | `theme`, `steeringMode`, `followUpMode` |
| 安全 | `blockImages` |

**AuthStorage：**
- API Key 存储（可加密文件）
- OAuth token 管理（含刷新机制）
- `setRuntimeApiKey()`（CLI `--api-key` 的运行时覆盖）

**ModelRegistry：**
- 从 `models.json` 发现可用 model
- 按 `(provider, modelId)` 查找/匹配
- 校验某 model 是否已配置认证
- 支持扩展动态注册 provider

**与框架的接口：** `AuthHook`（每次 LLM 调用前解析最新凭据），`ModelInfo`（来自 `llm-api-adapter` 的 `pub use`）

---

#### 5. 📦 Resource Loader

**TS 源码：** `core/resource-loader.ts`、`core/package-manager.ts`、`core/sdk.ts` 的 `createAgentSessionServices`

启动时从多个来源加载资源：

| 来源 | 路径 | 说明 |
|---|---|---|
| Skills 目录 | `.pi/skills/` + 配置额外路径 | `.md` 文件，带 frontmatter |
| Prompt Templates | `.pi/prompts/` + 配置额外路径 | 位置参数模板 |
| Extensions | `.pi/extensions/` + npm 包 | JS/TS 脚本 |
| Themes | `.pi/themes/` + 配置 | JSON 主题文件 |
| Context Files | `CLAUDE.md` / `AGENTS.md` | 项目级别的系统提示补充 |
| 包资源 | npm/git package | 通过 PackageManager 下载 |

**不因单个文件失败而整体失败：** 所有加载函数返回 `(成功列表, Vec<Diagnostic>)`。这是框架 Phase 6 的设计原则，应用层也遵循。

**与框架的接口：** 加载后的 resources 通过 `BeforeRunCtx.resources: &AgentHarnessResources` 传递给 Harness hooks。

---

#### 6. 🔌 Extension System（≈ 2000 行）

**TS 源码：** `core/extensions/`（types.ts > loader.ts > runner.ts > wrapper.ts > index.ts）

插件运行时，支持在编码 agent 启动后动态加载扩展。

**扩展的生命周期事件：**

```rust
enum ExtensionEvent {
    SessionStart { reason: Startup | Reload },
    SessionShutdown { reason: Shutdown | Reload },
    AgentStart,
    AgentEnd { messages },
    TurnStart { turn_index },
    TurnEnd { turn_index, message, tool_results },
    MessageStart { message },
    MessageEnd { message },
    ToolCall { ... },           // 拦截工具调用
    ToolResult { ... },         // 拦截工具结果
    Input { text, images },     // 拦截/转换用户输入
    BeforeProviderRequest { payload }, // 修改 LLM 请求
    ModelSelect { model, previous },
    ResourcesDiscover { cwd },  // 扩展声明资源路径
    Context { messages },       // 修改即将发送到 LLM 的上下文
    Command { name, args },     // slash 命令
    // ... 共约 25 个事件
}
```

**扩展可以做什么：**
- 注册自定义工具
- 注册 slash 命令
- 拦截/修改工具调用和结果
- 注入自定义消息到 LLM 上下文
- 自定义 Compaction（`BeforeCompactHook`）
- 在 TUI 上渲染自定义 UI 组件
- 动态提供 skill/prompt/theme 路径

**与框架的接口：** 主要映射到 `HarnessHooks` 的 11 个 hook。扩展系统是 HarnessHooks 的"多订阅者管理器"。框架的 hook 是单点（`Option<Arc<dyn Hook>>`），扩展系统允许多个扩展同时监听同一事件。

---

#### 7. 🎯 AgentSession（≈ 3000 行）

**TS 源码：** `core/agent-session.ts`

这是**从框架到应用最重要的单一块**。AgentSession 是应用层的核心编排者，它封装框架并添加业务逻辑。

```
AgentSession
  ├── AgentHarness   — 驱动 LLM 循环
  │                    (依赖 Phase 7)
  ├── SessionRepo    — 持久化 + 分支
  │                    (依赖 Phase 4)
  ├── Settings       — 配置管理
  ├── ModelRegistry  — 模型发现 + 认证校验
  ├── ResourceLoader — 资源加载
  ├── Tool Registry  — 7 个 coding tools 的注册/开关
  ├── Compaction 逻辑 — 自动（阈/overflow）+ 手动
  │                    (依赖 Phase 5)
  ├── Auto-Retry 逻辑 — 指数退避重试
  ├── Queue 管理     — steer/followUp 消息队列
  ├── Extension 绑定  — 扩展运行时
  └── System Prompt  — 按工具集动态重建
```

**核心业务逻辑（TS 流程翻译为 Rust）：**

```
async fn prompt(&self, text: &str) -> Result<()> {
    // 1. 如果是 "/command" → 交给 ExtensionRunner
    // 2. 如果包含 "/skill:name" → 展开 skill 内容
    // 3. 如果包含 "/template" → 展开 prompt template
    // 4. 如果正在 streaming → 用 steer()/followUp() 排队
    // 5. 验证 model + API key
    // 6. 调用 before_run hook
    // 7. 构造 LoopConfig → self.harness.prompt(messages)
    //
    // AgentHarness 返回后:
    // 8. 检查是否是 retryable 错误 → 指数退避 → continue
    // 9. 检查是否需要 compaction（threshold 或 overflow）→ compact → continue
    // 10. loop 直到无更多操作
}
```

**关键业务规则：**
- **Auto-Retry：** 指数退避 `baseDelayMs * 2^(attempt-1)`，默认 2s→4s→8s，最多 3 次。可重试：overloaded、rate limit、5xx、timeout。不可重试：context overflow（走 compaction）、quota/billing 错误
- **Auto-Compaction：** 两种触发——threshold（超过阈值后压缩但不回退）和 overflow（LLM 返回 context overflow 后压缩并自动 retry）
- **消息队列：** `steer`（当前 turn 工具执行完后插入）和 `followUp`（所有消息处理完后排队）
- **系统提示重建：** 工具集变更、资源重新加载后触发

**与框架的接口：** AgentSession 主要依赖 **AgentHarness**（Phase 7），而不是 Agent（Phase 3）。因为 AgentSession 需要 session 写入、hook 管线、compaction 编排——这些都是 AgentHarness 的职责。

---

#### 8. 🖥️ CLI 入口 + 3 种运行模式

**TS 源码：** `cli/args.ts` → `main.ts`（入口点）→ `modes/`

**参数解析（约 30+ 个 CLI 参数）：**

```
核心:
  --model, --provider        指定模型
  --session, --continue,     会话选择
    --resume, --fork
  --print, --mode rpc        运行模式
  --no-tools, --tools,       工具控制
    --exclude-tools
  --no-session               不持久化
  --system-prompt,           系统提示
    --append-system-prompt
  @file                      加载文件内容

资源:
  --extensions, --skills,
    --prompt-templates,
    --themes

辅助:
  --version, --help,
    --list-models
```

**三种运行模式：**

| 模式 | 用途 | 实现要点 |
|---|---|---|
| **Print** | 非交互，读 stdin → 一轮 LLM → 输出 stdout | 简单：`AgentHarness.prompt()` → 输出到 stdout |
| **Interactive** | 全屏 TUI | TUI 组件系统、事件驱动渲染、快捷键、主题 |
| **RPC** | JSON-RPC over stdio | JSON 消息编解码、并行请求、状态同步 |

**与框架的接口：** 主要在 `AgentHarness`（Phase 7）上构建。Interactive mode 还需要 `AgentEvent` / `AgentHarnessEvent` 的事件流来驱动 UI 渲染。

---

#### 9. 🎨 Terminal UI（TUI，≈ 8000+ 行估算）

**TS 源码：** `modes/interactive/`（约 40+ 个文件）

这是工作量最大的单一块。需要实现：

**组件系统：**
- 消息列表渲染器（用户消息、助手消息、系统消息各自不同样式）
- 工具执行状态渲染器（进度、中间结果、完成状态）
- 流式输出实时渲染（逐 token 显示）
- 分支选择器（树状展示 + LLM 生成摘要）
- 模型选择器（Ctrl+P 循环）
- 主题选择器、设置面板
- Footer（git 分支 + 扩展状态 + token 计数）
- 输入编辑器（多行输入、快捷键）

**主题系统：**
- `Theme` 结构体（前景色、背景色、强调色等）
- JSON 定义主题（深色/浅色）
- 动态切换

**快捷键系统：**
- 可配置的 keybinding 映射
- Chord 键支持（如 Ctrl+K, Ctrl+B）

**其他：**
- 剪贴板集成
- 图片渲染（iTerm2 / Kitty inline images）
- Git 信息集成
- `export-html`（会话导出为静态 HTML）

**与框架的接口：** `AgentEvent` + `AgentSessionEvent` 的事件流驱动整体 UI 渲染。TUI 不做业务决策，只做展示和输入收集。

---

#### 10. 🛠️ 辅助工具箱（≈ 2000 行）

| 模块 | 用途 |
|---|---|
| **Git 集成** | 读取 git 分支、文件变更状态——`FooterComponent` 依赖 |
| **剪贴板** | 跨平台复制粘贴（macOS pbcopy / Windows clip.xclip） |
| **图片处理** | resize（2000x2000 max）、格式转换（→PNG）、EXIF orientation |
| **粘贴板/`@file`** | 文件参数展开、stdin 读取 |
| **HTML 导出** | session → 带语法高亮的静态 HTML |
| **前端包管理** | npm 包发现、安装、资源加载 |
| **版本检查** | 自动更新检查 |
| **Telemetry** | 匿名安装统计 |

---

## 四、按层汇总工作量估算

### 框架层（Phase 3-7，全是 Rust）

| 阶段 | 模块 | 估算行数 | 构建顺序 |
|---|---|---|---|
| Phase 3 | Agent | ~600 | 1（依赖 Phase 1/2 完成） |
| Phase 4 | Session | ~1200 | 2（依赖 Phase 1） |
| Phase 5 | Compaction | ~400 | 3（依赖 Phase 4） |
| Phase 6 | Skills/Templates | ~400 | 4（依赖 Phase 1） |
| Phase 7 | AgentHarness | ~1500 | 5（依赖 Phase 2-6 全部） |
| **框架合计** | | **~4100 行** | |

### 应用层（Coding Agent，可以全部用 Rust 或混合）

| 模块 | 估算行数 | 构建顺序 |
|---|---|---|
| 7 个 Coding Tools | ~2500 | 可并行构建 |
| Tool Registry | ~300 | 依赖 Tools |
| System Prompt Builder | ~200 | 独立 |
| Settings + Auth + Model Registry | ~1500 | 可并行构建 |
| Resource Loader | ~600 | 依赖 Settings + Skills/Templates |
| Extension System | ~2000 | 依赖 Phases 3-7 全部 |
| AgentSession | ~3000 | 依赖以上全部 |
| CLI + 3 种运行模式 | ~1500 | 依赖 AgentSession |
| TUI | ~8000+ | 依赖 AgentSession + CLI |
| 辅助工具箱 | ~2000 | 独立，可逐步添加 |
| **应用合计** | | **~21,600 行** |
| **总计** | | **~25,700 行** |

> 注：TUI 部分占比约 37%，是最大的单一块。TS 参考实现中交互模式约 40+ 个组件，Rust 的实现如果直接用现有 TUI 库（如 `ratatui`）可能能少写一些，但不会少太多。

---

## 五、建议的实施顺序

### 第一阶段：完成框架（Phase 3-7）

```
Phase 3 (Agent) → 可用 MockLlmClient 测试
         ↓
Phase 4 (Session) → JSONL 持久化 + 分支
   ↓           ↓
Phase 5 (Compaction)   Phase 6 (Skills/Templates)
   ↓           ↓
Phase 7 (AgentHarness) ← 整合以上全部
```

**验证点：** 写一个集成测试：`prompt → tool call → session 写入 → compact → fork → navigate → 重建上下文`

### 第二阶段：实现具体 Tools + 基础设施

```
7 个 Coding Tools (Read/Bash/Edit/Write/Grep/Find/Ls)
Settings + Auth + Model Registry
System Prompt Builder
Resource Loader
```

**验证点：** 能通过 CLI 发起一次 `read` 工具调用，结果写入 session 并被重建

### 第三阶段：AgentSession + CLI + 简单 Print Mode

```
AgentSession（核心编排）
CLI 参数解析
Print Mode（非交互单轮）
```

**验证点：** `echo "list all files" | my-coding-agent --print` 能输出结果

### 第四阶段：Extension System + Interactive Mode

```
Extension System
TUI（交互模式）
RPC Mode
辅助工具箱
```

**验证点：** 项目能全功能启动，交互式对话、工具调用、session 持久化、fork/navigate、compaction 全部可用

---

## 六、关键架构决策回顾

以下决策来自框架 spec，对应用层设计有直接影响：

1. **AgentSession 依赖 AgentHarness，不是 Agent**——因为 AgentSession 需要 session 写入、hook 管线、compaction 编排
2. **消息历史的真相源是 Session**——AgentHarness 从 session 读取消息，不维护独立副本。消除了 TS 版的数据冗余同步问题
3. **Compaction 两段式**——`prepare` 在 types crate（纯函数），`execute` 在 harness crate（需要 LLM 调用）；AgentSession 不直接调 `compact()`，而是通过 `AgentHarness.compact()`
4. **工具是双层注册**——`ToolDefinition`（元数据）+ `AgentTool`（执行实例），AgentSession 管理工具注册/开关/过滤
5. **Extension System ≈ 多路 HarnessHooks**——框架的 HarnessHooks 是单点，扩展系统是多订阅者管理器，一个事件可以触发多个扩展的响应
6. **加载函数不因单个文件失败而整体失败**——`load_skills()` / `load_prompt_templates()` 返回 `(成功列表, Vec<Diagnostic>)`
