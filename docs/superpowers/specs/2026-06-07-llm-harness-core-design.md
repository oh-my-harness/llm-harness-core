# llm-harness-core 系统设计

**日期：** 2026-06-07
**状态：** 已批准（v6，纳入模块边界审核修订）

## 1. 背景与目标

本项目是 [`@earendil-works/pi-agent-core`](../../../../pi-main/packages/agent) TypeScript 包的 Rust 全量重写。目标不是逐行翻译，而是学习其核心设计哲学，用 Rust 惯用法重新表达。

核心目标：
- 提供完整的 agent 运行时：低层循环、有状态 Agent、编排层 AgentHarness
- 全量对标 pi-agent-core 功能：会话持久化（含分支前向兼容）、上下文压缩、skills/templates、执行环境抽象
- 以 [`llm-api-adapter`](../../../../llm-api-adapter) 作为 LLM provider 层

## 2. Crate 结构

Cargo workspace，三个 crate：

```
llm-harness-core/
├── Cargo.toml               (workspace)
└── crates/
    ├── llm-harness-types/   (纯类型 + trait)
    ├── llm-harness-loop/    (core loop 引擎)
    └── llm-harness/         (Agent + Harness + Session + Compaction + Skills)
```

依赖关系：

```
llm-api-adapter  (外部 crate)
      │
      ▼
llm-harness-types  ←──────────────────┐
      │                               │
      ▼                               │
llm-harness-loop               llm-harness
      │                               │
      └───────────────►───────────────┘
```

## 模块文档导航

| 阶段 | 模块 | 文档 |
|---|---|---|
| 阶段 1 | `llm-harness-types` | [→ types 设计](./2026-06-07-llm-harness-core-design-phase1-types.md) |
| 阶段 2 | `llm-harness-loop` | [→ loop 设计](./2026-06-07-llm-harness-core-design-phase2-loop.md) |
| 阶段 3 | Agent | [→ Agent 设计](./2026-06-07-llm-harness-core-design-phase3-agent.md) |
| 阶段 4 | Session | [→ Session 设计](./2026-06-07-llm-harness-core-design-phase4-session.md) |
| 阶段 5 | Compaction | [→ Compaction 设计](./2026-06-07-llm-harness-core-design-phase5-compaction.md) |
| 阶段 6 | Skills/Templates | [→ Skills 设计](./2026-06-07-llm-harness-core-design-phase6-skills.md) |
| 阶段 7 | AgentHarness | [→ AgentHarness 设计](./2026-06-07-llm-harness-core-design-phase7-agent-harness.md) |

## 6. 关键设计决策汇总

| 决策 | 选择 | 理由 |
|---|---|---|
| 事件模型 | 消息级 + token 级双层（含 AgentStart/End、MessageStart/End 携 payload） | 同时支持消息列表 UI 与字符流 UI；Agent↔Harness 通过 AgentEnd payload 传递结果 |
| ConvertToLlmHook 位置 | 定义在 `llm-harness-loop`（不在 types） | types crate 维持零 IO 约束 |
| Hook 真相源 | HarnessHooks 唯一源；Harness 每次构造临时 LoopConfig | 消除字段重复，明确翻译路径 |
| HookedTool | tool wrapper，承载 before/after_tool_call | Loop 层不接受 tool call hook |
| next_turn 注入 | HarnessState.queued_next_turn 缓冲，下次 prompt 时合并 | 无需新 channel |
| Steer/follow_up 类型 | `AgentMessage` channel + `&str` 便捷方法 | 支持多模态（图片等） |
| active_cursor 命名 | 弃用 active_leaf——fork 后会指向非 leaf | 命名准确反映"下次 append 的 parent" |
| Session::append 内部 | 自动填 id（storage.create_entry_id）/parent_id（active_cursor）/timestamp | 调用方零负担 |
| Session 并发 | Storage 内部 Mutex 串行化；高层组合操作持同一锁 | JSONL append + meta 更新原子 |
| JSONL 缓存 | Storage 内部维护内存树缓存，append 增量更新 | 避免每次树查询全量扫描 |
| CompactionSettings | 删除 token_threshold，触发条件统一为 context_window - reserve_tokens | 消除语义重叠；强制 summary_model_info |
| BuiltContext | build_context 返回 messages + 最后已知 model/thinking/tools | session 重建完整运行时配置 |
| 事件处理管道 | Harness 内部明确伪代码定义事件→pending writes→save point 流转 | 核心正确性可审计 |
| 消息富类型 | AssistantMessage 携 usage/timestamp/provider/model/error | compaction 估算、回放、错误处理需要 |
| ThinkingContent | 一等公民 ContentBlock variant | Anthropic extended thinking 必需，compaction 时保留思考 |
| Custom message | 框架内置 BranchSummary/CompactionSummary 为具名 variant；其他走 CustomMessage + 必需 ConvertToLlmHook | 类型安全 + 灵活扩展 |
| 工具定义 | `dyn Tool` trait，含 label / prepare_arguments / onUpdate channel / terminate | UI 友好 + LLM 参数兼容 + 流式工具输出 + 自主停止 |
| 工具调度 | 分治：按 Sequential 切子组，组内并发 | 避免连坐降级 |
| 执行环境 | 完整 trait（~13 方法）+ ShellOptions | 对齐 TS 的 FileSystem/Shell；路径操作走 std::path |
| 锁策略 | `std::sync::Mutex` + 快照模式 | 性能 + 跨 await 安全 |
| 阶段锁 | 运行时枚举 | typestate 不适合 async + 长生命周期 |
| Session 结构 | 真正的多分支树（parent_id + active_leaf + all_leaves） | v1 即支持 fork/navigate/list/delete 分支 |
| Session 仓库 | `SessionRepo` trait（create/open/list/delete/fork） | 多 session 管理；fork 跨 session 复制路径 |
| Session 读取 | `read_active_path` 原始 + `build_context` 解释 Compaction + `read_path_of(leaf)` 任意分支 | 双层职责 + 多分支支持 |
| 分支摘要 | `generate_branch_summary` 用 summary_model 生成；写入 BranchSummary entry | 导航 UI 显示分支概要 |
| Compaction | 基于 session entries；prepare/execute 两段；输出 first_kept_entry | 边界正确性 + 迭代摘要 |
| convert_to_llm | LoopConfig 必需 hook | CustomMessage 不能直送 LLM |
| prepare_next_turn | LoopConfig 可选 hook | Harness 从 session 重建上下文 |
| continue_run | Agent 与 loop 双层支持 | prepareNextTurn 基础 |
| AgentHarness 架构 | 直接驱动 loop，不包装 Agent | 对齐 TS 的"超集替代"定位 |
| Hook 数量 | 约 11 个语义化 hook + AgentHarnessEvent enum 通知 | 覆盖 TS 主要扩展点，避免事件膨胀 |
| 动态认证 | `AuthHook` 在 LoopConfig 与 Harness 均可挂入 | OAuth token 过期等场景 |
| StreamOptions | 显式结构传入 LoopConfig，可被 BeforeProviderRequestHook 覆盖 | 传输层配置可观测可修改 |
| Skill 加载 | 名称/描述校验 + disableModelInvocation + 显式调用 | 安全 + 灵活 |
| PromptTemplate | 位置参数 + shell-style 引号解析 | 对齐现有模板生态 |
| 图片引用 | `ImageSource` enum 预留 URL/Id | 避免 base64 锁死 |
| Crate 拆分 | workspace + 3 crates | 关注点分离 |
| HarnessPhase 位置 | types crate（与 HarnessError 同级） | HarnessError::NotIdle 需携带 HarnessPhase；避免跨 crate 引用编译错 |
| LlmClient 签名 | spec 中以注释形式给出假定接口 | loop 实施可直接开始，不阻塞于 llm-api-adapter 接口落地 |
| ModelInfo 暴露 | loop crate 通过 `pub use` 重导出 LlmClient/ModelInfo/LlmMessage/StreamEvent | harness 不直接依赖 llm-api-adapter |
| Skills/Templates 测试 | tempfile + OsEnv 真实 fs，无 InMemoryEnv | 避免维护双重 env 实现 |

## 7. 不在范围内（v1）

- **Proxy 模式**（browser → backend 流式转发）：可后续作为独立 feature
- **WASM 目标**：`ExecutionEnv` trait 已抽象；WASM 实现留待后期
- **后台 compaction**：v1 同步触发；v1.x 可考虑异步
- **细粒度权限模型**（capability token）：v1 由 ExecutionEnv 实现方控制
- **超大 entry 外部存储**：v1 调用方截断
- **`prepareArguments` 的 typed schema 泛型**：v1 用 `serde_json::Value`，未来可引入 typed 泛型 Tool
- **Agent loop 以外的框架能力**（规划、记忆管理）：调用方责任
- **典型应用层组件**（如内置 bash tool / file edit tool）：作为示例代码或独立 crate 提供
- **分支可视化 UI**：框架提供数据接口（`list_branches`、tree 查询），UI 由调用方实现

## 8. 实施路线图

依赖拓扑：`types → loop → harness`。harness 内部进一步分为可并行的子系统。

```
阶段 1 (types) ─────────────────────────────────────────────────
                │
                ▼
阶段 2 (loop) ──────────────────────────────────────────────────
                │
                ▼
        ┌───────┴──────────┬──────────────────┐
        ▼                  ▼                  ▼
阶段 3 (Agent)    阶段 4 (Session)    阶段 6 (Skills/Templates)
        │                  │                  │
        │                  ▼                  │
        │          阶段 5 (Compaction)        │
        │                  │                  │
        └──────────────────┴──────────────────┘
                           │
                           ▼
                阶段 7 (AgentHarness)
```

| 阶段 | 内容 | 关键测试 | 阻塞依赖 |
|---|---|---|---|
| 1 | types 全部类型 + trait 声明 | 纯数据 derive 与序列化 roundtrip | 无 |
| 2 | LoopConfig / HookedTool / agent_loop / agent_loop_continue / DefaultConvertToLlm | `MockLlmClient` 驱动的事件序列正确性 + tool batch 分治 | 阶段 1 完成 |
| 3 | Agent / AgentState / 队列方法 / continue_run / reset | 状态机转换、abort、并发 prompt、subscribe 完整事件 | 阶段 2 完成 |
| 4 | SessionEntry / SessionStorage / SessionRepo / Session / 内存 + JSONL 实现 / 分支操作 | 树操作、fork、navigate、build_context、并发 append | 阶段 1 完成 |
| 5 | CompactionSettings / prepare_compaction / compact / FileOperation | 纯函数 cut point + MockLlmClient 集成 | 阶段 4 数据结构稳定 |
| 6 | Skills / PromptTemplates / load_skills / invoke_template | tempfile + OsEnv 真实 fs 加载 | 阶段 1 完成 |
| 7 | AgentHarness / HarnessHooks / 事件管道 / 分支 API / next_turn 注入 | 集成测试：prompt → tool → session 写入 → compact → fork → navigate | 阶段 2-6 全部完成 |

**并行机会：** 阶段 3、4、6 可在阶段 2 完成后同时启动（三者互不依赖）。阶段 5 在阶段 4 的 SessionEntry / SessionStorage 数据结构定稿后即可启动。

**测试基础设施（实施任务，不进 spec 主体）：**
- loop crate 提供 feature-gated `test-utils` 模块导出 `MockLlmClient`
- harness 集成测试用 `InMemorySessionRepo` + `MockLlmClient` + `tempfile::TempDir`
