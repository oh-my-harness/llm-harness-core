# Session 模型

Core session 是只追加的 entry tree。一个 session 会存储 messages、
配置变更、compaction summaries、branch metadata 和自定义 entries。

`AgentHarness` 以 session state 作为事实来源。每次运行 loop 之前，它都会从
active path 重新构建有效上下文。

## 核心概念

- `SessionEntry`：tree 中的一个节点。
- `parent_id`：指向 branch 上的前一个 entry。
- active cursor：下一次 append 会挂到的 entry。
- branch：entry tree 中的一条路径。
- `BuiltContext`：有效 messages，加上最后已知的 model、thinking level 和
  active tools。

## 常见操作

- `append_message`：追加 user、assistant 或 tool-result message。
- `fork_branch`：从已有 entry 创建新 branch。
- `navigate_to`：把 active cursor 移动到另一个 entry。
- `list_branches`：查看可用的 branch leaves。
- `build_context`：为 active path 重新构建有效上下文。

## Compaction

Compaction 会写入一个 summary entry，用它替代未来运行中的较早上下文。
`build_context` 会解释 compaction entries，因此调用方不需要手动过滤历史。

## Storage

Core 内置了 in-memory 和 JSONL-backed repositories。如果需要不同的后端，
实现 `SessionStorage` 和 `SessionRepo`。
