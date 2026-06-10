# Session Model

Core sessions are append-only entry trees. A session stores messages,
configuration changes, compaction summaries, branch metadata, and custom
entries.

`AgentHarness` uses session state as the source of truth. It rebuilds the
effective context from the active path before running the loop.

## Key Concepts

- `SessionEntry`: one node in the tree.
- `parent_id`: points to the previous entry on the branch.
- active cursor: the entry that the next append will attach to.
- branch: a path through the entry tree.
- `BuiltContext`: effective messages plus last known model, thinking level, and
  active tools.

## Common Operations

- `append_message`: append a user, assistant, or tool-result message.
- `fork_branch`: create a new branch from an existing entry.
- `navigate_to`: move the active cursor to another entry.
- `list_branches`: inspect available branch leaves.
- `build_context`: rebuild the effective context for the active path.

## Compaction

Compaction writes a summary entry that replaces older context for future runs.
`build_context` interprets compaction entries so callers do not need to manually
filter history.

## Storage

Core includes in-memory and JSONL-backed repositories. Implement
`SessionStorage` and `SessionRepo` when you need a different backend.

