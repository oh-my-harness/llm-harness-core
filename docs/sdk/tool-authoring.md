# Tool 编写

Tool 是 core 执行外部动作的唯一方式。一个 tool 需要实现
`llm-harness-types` 中的 `Tool` trait。

Core 不关心 tool 的具体业务行为。它只需要：

- 稳定的名称。
- 用于 LLM tool definition 的描述。
- 参数的 JSON schema。
- 一个异步的 `execute` 实现。

## 最小 Tool

```rust
use futures::future::BoxFuture;
use llm_harness_types::{
    ContentBlock, Tool, ToolContext, ToolError, ToolResult,
};
use serde_json::json;

struct EchoTool {
    schema: serde_json::Value,
}

impl EchoTool {
    fn new() -> Self {
        Self {
            schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
        }
    }
}

impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo text back to the assistant."
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        &self.schema
    }

    fn execute<'a>(
        &'a self,
        args: serde_json::Value,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let text = args["text"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidArguments("text required".into()))?;

            Ok(ToolResult {
                content: vec![ContentBlock::Text { text: text.into() }],
                details: serde_json::Value::Null,
                terminate: false,
            })
        })
    }
}
```

## ToolContext

`ToolContext` 提供：

- `env`：文件系统和 shell 抽象。
- `abort`：取消信号。
- `tool_use_id`：来自 LLM tool call 的 ID。
- `turn_index`：当前 turn 编号。
- `assistant_message`：请求 tool call 的 assistant 消息。
- `update_tx`：可选的增量 tool result 输出通道。

长时间运行的 tool 应该检查 `ctx.abort`，并在收到取消请求时返回
`ToolError::Aborted`。

## Execution Mode

Tools 默认并行执行。如果某个 tool 必须成为工具批次调度中的边界，可以覆盖
`execution_mode`，并返回 `ToolExecutionMode::Sequential`。
