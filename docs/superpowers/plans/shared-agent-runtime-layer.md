# 共享 Agent 运行时层分析

> ⚠️ **初步分析，非最终设计**
>
> 本文是基于阅读 TS 参考实现 `pi-coding-agent` 的初步分析，
> 目的是识别 coding agent 和 EDA agent 之间的公共部分。
> 文档中的抽象设计（`DomainConfig`、`SystemPromptStrategy` 等）是概念推演，
> 不是经过验证的 API 设计。在开始实现共享层之前，需要重新审视和验证
> 这些抽象的合理性。当前内容仅供参考和讨论。

**日期：** 2026-06-07
**目标：** 识别 coding agent 和 EDA agent 在框架之上的公共部分，设计可复用的共享运行时层

---

## 一、核心洞察：基础工具本身就是共享的

**纠正一个关键误区：** 之前我以为 Read/Bash/Edit/Write/Grep/Find/Ls 是 coding agent 特有的。

实际上 EDA agent 同样需要：
- **Read** → 读设计网表、时序报告、log 文件
- **Bash** → 跑 EDA 工具的 TCL 脚本、调命令行工具
- **Edit** → 修改配置文件、TCL 脚本、约束文件
- **Write** → 写新的脚本、报告
- **Grep** → 搜索 log 中的错误/警告
- **Find/Ls** → 查找设计文件目录结构

**这些基础工具是领域无关的。** EDA agent 额外需要的是 EDA 工具专属的 Tool（如 `compile_ultra`、`report_timing`）。

---

## 二、修正后的三层架构

```
┌──────────────────────────────────────────────────────────────┐
│  域特有层（Domain Extensions）                                │
│  ┌────────────────────┐  ┌──────────────────────────────┐   │
│  │ coding-agent       │  │ eda-agent                    │   │
│  │ "You are an        │  │ "You are an EDA              │   │
│  │  expert coder"     │  │  assistant"                  │   │
│  │ (无额外领域工具)     │  │ compile_ultra/report_timing │   │
│  │                    │  │ 领域 session 管理器 + MCP    │   │
│  │ TUI 入口            │  │ HTTP API 入口                │   │
│  └────────────────────┘  └──────────────────────────────┘   │
├──────────────────────────────────────────────────────────────┤
│  共享运行时层 + 基础工具（AgentRuntime + BasicTools）          │
│                                                              │
│  ┌─ orchestration ────────────────────────────────────┐     │
│  │ prompt 生命周期 / 工具注册 / 模型管理 / 自动重试    │     │
│  │ compaction 编排 / 事件总线 / 资源加载 / 扩展系统   │     │
│  │ settings/auth / CLI 基础设施                       │     │
│  └───────────────────────────────────────────────────┘     │
│                                                              │
│  ┌─ basic tools ──────────────────────────────────────┐     │
│  │ Read / Bash / Edit / Write / Grep / Find / Ls      │     │
│  │ (coding agent 和 EDA agent 都用同一套实现)          │     │
│  └───────────────────────────────────────────────────┘     │
├──────────────────────────────────────────────────────────────┤
│  llm-harness-core (Phase 1-7)                               │
│  types / loop / Agent / Session / Compaction / Skills       │
│  / AgentHarness                                             │
└──────────────────────────────────────────────────────────────┘
```

---

## 三、逐组件分析：能共享 vs 不能共享

### ✅ 完全共享（放入共享层 + 基础工具层）

| 组件 | 说明 |
|---|---|
| **Prompt 生命周期** | 验证 model → 验证 API key → check compaction → call harness → check retry → loop |
| **消息队列** | steer/followUp 纯队列逻辑 |
| **Skill 展开** | `/skill:name args` 格式通用 |
| **Template 展开** | 位置参数模板格式通用 |
| **Tool Registry** | 管理 `Vec<Arc<dyn Tool>>`，不关心具体工具做什么 |
| **模型管理** | set/cycle model、thinking level clamp——通用 |
| **Compaction 编排** | threshold/overflow 检测 + compact + retry |
| **自动重试** | 指数退避（2s→4s→8s），可重试/不可重试分类 |
| **Settings Manager** | 配置分层加载（项目级 + 全局级）、锁文件 |
| **Auth Storage** | API key 存储、OAuth token 管理 |
| **Model Registry** | 模型发现、认证校验 |
| **Resource Loader** | 加载 skills/prompts/templates——逻辑通用 |
| **Session Stats/Export** | token 统计、HTML 导出 |
| **事件总线** | 纯通知机制 |
| **基础工具** | **Read / Bash / Edit / Write / Grep / Find / Ls**——领域无关 |

### ❌ 领域特有（每个 agent 自己实现）

| 组件 | Coding Agent | EDA Agent | 差异本质 |
|---|---|---|---|
| **额外 Tool 实现** | 无（基础工具已够） | `compile_ultra` / `report_timing` / `set_propagated_clock`... | EDA 工具特有的原子操作 |
| **System Prompt 内容** | "You are an expert coding assistant" | "You are an EDA assistant" | 领域身份不同 |
| **Skill 内容 (.md)** | 编码规范、git 工作流 | EDA 流程（ECO 修复、库优化） | 领域知识不同 |
| **有状态后端管理** | 不需要（bash 即用即弃） | 需要保持 EDA 工具后台进程、支持 checkpoint/rollback | 执行模型不同 |
| **MCP Server** | 通常不需要 | FluxEDA 用 MCP 暴露工具能力 | 对外接口不同 |
| **入口形式** | CLI + TUI | HTTP API / Webhook / MCP Server | 使用场景不同 |
| **额外 Settings 字段** | shell 前缀、图片设置 | EDA 工具路径、license 配置 | 领域配置不同 |

### ⚠️ 部分领域相关（通过策略/配置抽象）

| 组件 | 共享部分 | 差异部分 | 抽象方式 |
|---|---|---|---|
| **System Prompt 框架** | 拼接规则：工具列表 + guidelines + 上下文 + skills + 日期 + cwd | 领域身份段落 | `SystemPromptStrategy` trait |
| **工具集组合** | 注册/过滤/开关注册表 | 默认工具列表（基础 + 领域特有） | `DomainConfig.default_tools` |
| **CLI 框架** | 核心参数（`--model`、`--session`、`--no-tools`） | 领域特有参数 | CLI 扩展点 |
| **TUI 组件** | 消息列表、模型选择器、设置面板、footer | 工具执行结果渲染 | `Tool::render_result()` 可选方法 |
| **Tool 执行渲染** | ToolExecutionComponent 框架 | 每个工具的输出展示 | `Tool::render_result()` |

---

## 四、抽象设计：两个核心 trait + 一个配置结构

### `DomainConfig`——领域配置文件

```rust
/// 每个领域在初始化时提供此配置，AgentRuntime 据此参数化行为。
pub struct DomainConfig {
    // ── 必需 ──
    /// 领域名称（用于日志、事件标识）
    pub name: String,

    /// 领域的默认工具集（基础工具 + 领域特有工具）
    pub default_tools: Vec<Arc<dyn Tool>>,

    /// System Prompt 构建策略
    pub system_prompt_builder: Arc<dyn SystemPromptStrategy>,

    // ── 可选 ──
    /// 默认启用的工具名（None = 全部启用）
    pub default_active_tools: Option<HashSet<String>>,

    /// CLI 参数扩展（None = 仅使用通用参数）
    pub cli_flags: Option<Vec<CliFlagDefinition>>,

    /// 领域特有的 Settings schema（None = 仅通用 settings）
    pub extra_settings: Option<Vec<SettingDefinition>>,

    /// 资源默认路径
    pub resource_paths: ResourcePaths,

    /// 入口类型
    pub entry_point: EntryPoint,
}

/// 提供基础工具的工厂函数（共享层内置）
impl DomainConfig {
    pub fn with_basic_tools() -> Vec<Arc<dyn Tool>> {
        vec![
            Arc::new(ReadTool::new()),
            Arc::new(BashTool::new()),
            Arc::new(EditTool::new()),
            Arc::new(WriteTool::new()),
            Arc::new(GrepTool::new()),
            Arc::new(FindTool::new()),
            Arc::new(LsTool::new()),
        ]
    }
}
```

### `SystemPromptStrategy` trait——系统提示构建

```rust
/// 领域自己定义如何构建 system prompt。
/// 共享层提供公共的 context，领域决定自己的身份描述。
pub trait SystemPromptStrategy: Send + Sync {
    fn build(&self, context: PromptBuildContext) -> String;
}

pub struct PromptBuildContext {
    /// 当前启用的工具列表及 prompt_snippet
    pub tool_snippets: Vec<(String, String)>,
    /// 工具行为准则
    pub prompt_guidelines: Vec<String>,
    /// 项目上下文文件（如 CLAUDE.md）
    pub context_files: Vec<(String, String)>,
    /// 加载的 Skills
    pub skills: Vec<Skill>,
    /// 工作目录
    pub cwd: String,
    /// 当前日期
    pub date: String,
    /// 用户配置的追加提示
    pub append_prompt: Option<String>,
}
```

### `ToolProvider` trait——工具提供者

```rust
/// 领域提供其领域特有的工具集。
/// 基础工具由共享层内置，ToolProvider 只补充领域额外的工具。
pub trait ToolProvider: Send + Sync {
    /// 返回领域特有的工具（不含基础工具）
    fn extra_tools(&self) -> Vec<Arc<dyn Tool>>;

    /// 可选：工具变更通知（MCP server 动态增减）
    fn on_tools_changed(&self) -> Option<tokio::sync::watch::Receiver<()>> {
        None
    }
}
```

---

## 五、两个 agent 的代码对比

### Coding Agent

```rust
// 只需要 System Prompt 策略——基础工具直接复用共享层
struct CodingSystemPrompt;
impl SystemPromptStrategy for CodingSystemPrompt {
    fn build(&self, ctx: PromptBuildContext) -> String {
        format!(
            "You are an expert coding assistant operating inside pi...\n\
             Available tools:\n{}\nGuidelines:\n{}\nCurrent date: {}\nCurrent cwd: {}",
            format_tool_snippets(&ctx.tool_snippets),
            format_guidelines(&ctx.prompt_guidelines),
            ctx.date, ctx.cwd,
        )
    }
}

// 无领域特有工具，无 ToolProvider
fn main() {
    let config = DomainConfig {
        name: "coding-agent".into(),
        default_tools: DomainConfig::with_basic_tools(), // ← 只有基础工具
        system_prompt_builder: Arc::new(CodingSystemPrompt),
        entry_point: EntryPoint::Tui,
        ..Default::default()
    };
    let harness = AgentHarness::new(/* ... */);
    let runtime = AgentRuntime::new(config, harness);
    run_tui(runtime);
}
```

### EDA Agent

```rust
// System Prompt 策略——领域身份 + 额外工具的描述
struct EdaSystemPrompt;
impl SystemPromptStrategy for EdaSystemPrompt {
    fn build(&self, ctx: PromptBuildContext) -> String {
        format!(
            "You are an EDA assistant specialized in chip design...\n\
             Available tools:\n{}\nGuidelines:\n{}\nCurrent date: {}\nCurrent cwd: {}",
            format_tool_snippets(&ctx.tool_snippets),
            format_guidelines(&ctx.prompt_guidelines),
            ctx.date, ctx.cwd,
        )
    }
}

// 领域特有工具
struct CompileUltraTool;
impl Tool for CompileUltraTool { /* ... */ }
struct ReportTimingTool;
impl Tool for ReportTimingTool { /* ... */ }
struct SetPropagatedClockTool;
impl Tool for SetPropagatedClockTool { /* ... */ }

fn main() {
    let config = DomainConfig {
        name: "eda-agent".into(),
        default_tools: {
            let mut tools = DomainConfig::with_basic_tools();  // ← 基础工具
            tools.push(Arc::new(CompileUltraTool));             // ← 领域特有
            tools.push(Arc::new(ReportTimingTool));
            tools.push(Arc::new(SetPropagatedClockTool));
            tools
        },
        system_prompt_builder: Arc::new(EdaSystemPrompt),
        entry_point: EntryPoint::Api(vec!["0.0.0.0:8080".into()]),
        ..Default::default()
    };
    let harness = AgentHarness::new(/* ... */);
    let runtime = AgentRuntime::new(config, harness);
    run_api_server(runtime);
}
```

---

## 六、复用率定量估算（修正版）

| 层面 | 内容 | 代码量 | 可复用程度 |
|---|---|---|---|
| Framework (Phase 3-7) | Agent + Session + Compaction + Skills + AgentHarness | ~4,100 行 | 框架固定 |
| **共享运行时** | prompt 生命周期 / retry / compaction / settings / auth / registry / 事件 / 扩展系统 | **~5,000 行** | **所有 agent 共享** |
| **基础工具** | Read / Bash / Edit / Write / Grep / Find / Ls | **~2,500 行** | **所有 agent 共享** |
| Coding Agent 特有 | System Prompt 内容 + TUI | ~500 行 | coding 领域 |
| EDA Agent 特有 | EDA 工具适配器 + 有状态后端管理 + MCP + System Prompt | ~10,000 行 | EDA 领域 |

**结论：**
- coding agent 只需要写 ~500 行领域代码（system prompt + TUI 配置）
- EDA agent 需要写 ~10,000 行领域代码（EDA 工具适配器是主要工作量）
- **共享层 + 基础工具共 ~7,500 行代码对所有 agent 复用**

---

## 七、与 OpenClaw 架构的对应

```
OpenClaw                            ↔  llm-harness-runtime + 基础工具
  Skills 引擎                       ↔    Skills (Phase 6) + 展开逻辑（runtime）
  MCP 集成                          ↔    dyn Tool + 可选的 MCP 适配器
  记忆/持久化                       ↔    Session (Phase 4) + settings
  Channel 抽象（终端/微信等）         ↔    EntryPoint
  插件系统                          ↔    Extension System（runtime）
  文件/Shell 基本操作                ↔    7 个基础工具

FluxEDA                             ↔  eda-agent（域特有层）
  EDA 工具适配器                    ↔    impl Tool for CompileUltra 等
  EDA Skills                        ↔    EDA 流程的 .md 文件
  TCL Gateway / RPC                 ↔    Tool::execute() 内的通信逻辑
  状态沙箱                          ↔    EDA 工具的 session 保持 + rollback
```
