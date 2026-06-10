# Tool Authoring

Tools are the only way core executes external actions. A tool implements the
`Tool` trait from `llm-harness-types`.

Core does not know what a tool does. It only needs:

- A stable name.
- A description for the LLM tool definition.
- A JSON schema for arguments.
- An async `execute` implementation.

## Minimal Tool

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

`ToolContext` provides:

- `env`: file system and shell abstraction.
- `abort`: cancellation signal.
- `tool_use_id`: ID from the LLM tool call.
- `turn_index`: current turn number.
- `assistant_message`: message that requested the tool call.
- `update_tx`: optional stream of partial tool results.

Long-running tools should check `ctx.abort` and return `ToolError::Aborted`
when cancellation is requested.

## Execution Mode

Tools are parallel by default. Override `execution_mode` and return
`ToolExecutionMode::Sequential` when a tool must be a boundary in the tool batch
schedule.

