/// Harness 运行时资源——由 `BeforeRunHook` 的上下文携带，供 hook 访问。
///
/// 具体字段在 Phase 7（AgentHarness）中填充；此处为 stub，
/// 使 types crate 中的 hook trait 可以编译。
pub struct AgentHarnessResources {
    _private: (),
}
