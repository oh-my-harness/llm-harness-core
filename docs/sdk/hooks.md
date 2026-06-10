# Hooks

Hooks let callers customize behavior without changing core loop or harness
logic. They are grouped by when they run and whether they can modify behavior.

## Harness Hooks

`HarnessHooks` is the single configuration object for harness-level hooks. The
harness builds a temporary `LoopConfig` from hooks and current state before each
run.

Common hook categories:

- Run and turn hooks: observe or prepare lifecycle boundaries.
- Context hooks: transform messages before they are sent to the provider.
- Tool hooks: allow, modify, deny, or patch tool execution.
- Provider hooks: observe or customize request-level behavior.
- Compaction hooks: decide or prepare compaction behavior.
- Stop hooks: decide whether the loop should continue.

## Events vs Hooks

Events are notifications. Subscribe to `AgentEvent` or `AgentHarnessEvent` when
you need UI updates, logs, telemetry, or progress rendering.

Hooks are behavior controls. Use hooks when you need to modify context, tool
arguments, tool results, or run decisions.

## Runtime Layers

A runtime or product layer can multiplex many plugins/extensions into the core
hook set. Core intentionally exposes typed hook points but does not own a plugin
runtime.

