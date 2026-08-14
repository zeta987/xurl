# Agy Provider Design Notes

## Status

Implemented for main-thread read, query, and write.

Current unsupported areas:

- role-based create
- child/subagent drill-down

## Purpose

This document records the verified Google Antigravity CLI (`agy`) behavior that
`xurl` relies on today and the current integration contract for the `agy`
provider. Every storage claim below was measured against a local `agy` v1.1.13
install; anything still unverified is called out explicitly.

## Implemented Provider Contract

### Supported URI Forms

- `agents://agy`
- `agents://agy/<conversation_id>`
- `agents://agy?q=<keyword>`
- `agents:///path?providers=agy`
- `agents://agy -d "..."`
- `agents://agy/<conversation_id> -d "..."`

### Unsupported URI Forms

- `agents://agy/<role> -d "..."`
- `agents://agy/<conversation_id>/<child_id>`

Both cases return explicit provider errors instead of guessing Antigravity
semantics.

### Root Resolution

`xurl` resolves the agy root in this order:

1. `AGY_HOME`
2. `GEMINI_CLI_HOME/.gemini/antigravity-cli`
3. `~/.gemini/antigravity-cli`

Conversations are read from:

- `<root>/conversations/<conversation_id>.db`

Note the counter-intuitive location: the CLI stores its data under `~/.gemini`,
not `~/.antigravity`. `~/.antigravity` and `%LOCALAPPDATA%\antigravity` belong to
the Antigravity IDE (a VS Code fork) and are unrelated to the CLI. No
`AGY_HOME`-style variable was observed in the binary; `AGY_HOME` is an `xurl`
convention that follows the `CODEX_HOME`/`COPILOT_HOME` precedent.

## Verified Storage Model

### 1. One SQLite Database per Conversation

```sql
CREATE TABLE `trajectory_meta` (
  `trajectory_id` text, `cascade_id` text,
  `trajectory_type` integer, `source` integer,
  PRIMARY KEY (`trajectory_id`));

CREATE TABLE `steps` (
  `idx` integer, `step_type` integer NOT NULL DEFAULT 0,
  `status` integer NOT NULL DEFAULT 0,
  `has_subtrajectory` numeric NOT NULL DEFAULT false,
  `metadata` blob, `error_details` blob, `permissions` blob,
  `task_details` blob, `render_info` blob,
  `step_payload` blob, `step_format` integer NOT NULL DEFAULT 0,
  PRIMARY KEY (`idx`));
```

`cascade_id` equals the conversation id and the database file name.
`step_format` was `0` for every row observed.

### 2. `step_payload` Is Schema-Less Protobuf

Each payload shares a common envelope and then carries one type-specific body in
a field whose number varies per step type:

- field 1 (varint): mirrors the `step_type` column
- field 4 (varint): mirrors the `status` column
- field 5: envelope with `google.protobuf.Timestamp` sub-messages, token counts,
  and the `trajectory_id`/`cascade_id` pair

Measured body layouts:

| `step_type` | body field | content |
|---|---|---|
| 14 | 19 | user input; prompt text at sub-field 2 |
| 15 | 20 | agent response; visible text at sub-field 1, reasoning at sub-field 3, tool call at sub-field 7 |
| 23 | 30 | task summary; conversation title at sub-field 4 |
| 8 | 14 | file-read tool result |
| 9 | 15 | directory-listing tool result |
| 7 / 21 | 31 (+13 / +28) | tool error details |
| 98 / 101 | 111 / 114 | bookkeeping events |

### 3. `step_type` Is Open-Ended, So It Is Not Enumerated

Types 8 and 9 are both tool results with different payload shapes, and log
strings such as `CORTEX_STEP_TYPE_GREP_SEARCH` and `CORTEX_STEP_TYPE_RUN_COMMAND`
confirm the column is effectively one value per tool. Types 21 and 101 appeared
only in some conversations.

`xurl` therefore decodes only the three types it needs (14, 15, 23) and skips
everything else instead of hard-coding an exhaustive enum that would break as
Antigravity adds tools.

### 4. Visible Text and Reasoning Are Separate Fields

Within the agent-response body, sub-field 1 holds user-visible prose and
sub-field 3 holds private model reasoning. Sub-field 8 duplicates sub-field 1
(verified identical in every observed row).

`xurl` renders sub-field 1 only, so reasoning never reaches rendered output or
`q=` search text. This matches the existing Cursor contract.

Many agent steps carry no sub-field 1 at all: the turn was purely a tool call.
Those steps produce no message, which is why a long tool-heavy conversation can
correctly render only a couple of visible messages.

### 5. Large Payloads Contain an Opaque High-Entropy Blob

Tool-call bodies embed a byte run (for example at `20.7.7.2.1`) that is not
protobuf. Measured: 7.95 bits/byte entropy, all 256 byte values present, no magic
bytes, and zlib/raw-deflate/gzip/bzip2/LZMA all fail at every tested offset. It is
encrypted or already-compressed with no recoverable header.

`xurl` does not attempt to decode it. The readable parts of a tool call (tool
name and argument JSON) are plain fields and remain accessible, so rendering
degrades gracefully rather than failing.

### 6. The Write-Ahead Log Holds the Newest Messages

For an active conversation the `.db` can be stale while the `-wal` holds recent
turns. Measured on conversation `1c15ca1c-...`:

- `.db` opened alone: `max(idx)=3`, 4 rows, step types `[14, 15, 23, 98]`
- `.db` opened with its `-wal`: `max(idx)=6`, 7 rows, step types `[14, 15, 23, 98, 101]`

Reading only the `.db` silently drops the most recent turns.

## Read and Query Strategy

### Staged Copy Instead of In-Place Open

Unlike `cursor.rs`, which opens `store.db` in place, the agy provider copies the
`.db` and its `-wal` into a private staging directory under
`%TEMP%/xurl-agy/<root-key>/` and opens the copy read-only.

This is a deliberate divergence from the Cursor precedent, for a measured reason:
opening a WAL-mode database read-only in place causes SQLite to create `-shm` and
empty `-wal` files inside the user's data directory. Copying keeps the read fully
non-mutating while still replaying the log. The `-shm` is intentionally not
copied, because SQLite rebuilds it from the `-wal` and a stale copy can
misrepresent the log contents.

Verified: reading a conversation through `xurl` leaves file names, sizes, and
mtimes in `<root>/conversations/` byte-for-byte unchanged.

### Message Reconstruction

Steps are read in `idx` order and mapped to a materialized JSONL view that reuses
the existing Cursor/OpenCode message shape, so the shared renderer and query
pipeline need no agy-specific logic.

### Metadata

Thread metadata is assembled from two sources:

- the task-summary step (`step_type` 23) supplies the conversation title
- `<root>/cache/conversation_metadata.json` supplies `Preview`, `UpdatedAt`, and
  `WorkspaceURIs`, keyed by conversation id

`WorkspaceURIs` provides `scope_path` for path-scoped queries. It is frequently
`null`, in which case the conversation simply has no scope path.

## Write Strategy

### CLI Entry Points

Verified working:

```
agy -p "<prompt>" --output-format stream-json                        # create
agy -p "<prompt>" --conversation <id> --output-format stream-json    # append
```

Omitting `--conversation` starts a new conversation whose id arrives in the
`init` event.

### Stream Semantics

Measured event shapes:

- `{"event":"init","conversation_id":"...","init":{...}}`
- `{"event":"step_update","step_update":{"step_index":N,"state":"ACTIVE|DONE","step_type":"agent_response","text_delta":"..."}}`
- `{"event":"result","result":{"conversation_id":"...","status":"SUCCESS","response":"...","usage":{...}}}`

Two behaviors drive the implementation:

1. **`DONE` repeats the whole text of a step.** Emitting every `text_delta`
   verbatim would print the answer twice. `xurl` tracks emitted text per
   `step_index` and forwards only the new suffix.
2. **Deltas can split multi-byte characters.** An observed chunk ended
   mid-codepoint, so streamed deltas are display-only; the authoritative final
   text is taken from `result.response`.

Only `step_type == "agent_response"` updates are forwarded, so system and
checkpoint steps do not leak into output. Observed named step types include
`user_input`, `system_message`, `agent_response`, `checkpoint`, and `unknown`.

Role-based create stays unsupported because Antigravity's `--agent`/`--mode`
flags do not map to a stable `xurl` role concept.

## Subagent Status

`steps.has_subtrajectory` and the separate `trajectory_id` suggest Antigravity
models subagents internally, but no parent-child contract was verified: every
local conversation had exactly one `trajectory_meta` row and `parent_references`
was empty in all samples.

`xurl` therefore returns `UnsupportedSubagentProvider` for agy drill-down,
matching the Cursor arm rather than Kimi's empty-list arm.

## Test Fixture Deviation

`AGENTS.md` asks for a sanitized copy of a real thread under
`xurl-cli/tests/fixtures/<provider>_real_sanitized/`. The agy fixture is
**generated programmatically** instead, by `setup_agy_tree()` in
`xurl-cli/tests/cli.rs`.

Reason: agy stores conversation content inside opaque protobuf blobs in SQLite.
Sanitizing string content in place is not reliable there, and shipping a real
database would risk publishing private content to a public repository. The
generated database reproduces the real schema and the real step layout, so it
exercises the same parsing paths. `manifest.json` documents the convention and
the ids used.

## Constraints and Follow-Ups

- The `step_type` to body-field mapping is measured, not documented by Google. If
  Antigravity changes the wire layout, message extraction degrades to producing
  no messages rather than producing wrong ones.
- Token usage is present in the step envelope and in `result.usage`, but is not
  currently surfaced in frontmatter.
- Per-conversation model name has no persisted record; only a global default in
  `settings.json` and a transient log line were found.
- If a verified parent-child contract appears, subagent support can be added on
  top of this provider without changing main-thread URI behavior.
