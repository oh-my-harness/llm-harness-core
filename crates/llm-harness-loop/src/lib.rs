pub mod config;
pub mod convert;
pub mod dispatch;
pub mod hooked_tool;
pub mod loop_fn;
pub mod stream_state;

mod type_bridge;

#[cfg(feature = "test-utils")]
pub mod test_utils;

// Top-level re-exports
pub use config::LoopConfig;
pub use convert::{ConvertToLlmHook, CustomMessageConverter, DefaultConvertToLlm};
pub use hooked_tool::HookedTool;
pub use loop_fn::{agent_loop, agent_loop_continue};

// Re-exports for downstream crates (harness must not depend directly on llm_adapter)
pub use llm_adapter::LlmError;
pub use llm_adapter::provider::Provider as LlmClient;
pub use llm_adapter::types::Message as LlmMessage;
pub use llm_adapter::types::StreamEvent as AdapterStreamEvent;
