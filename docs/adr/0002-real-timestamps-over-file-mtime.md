# Query listings use real conversation timestamps, not file mtime

Every provider's `updated_at` in a query listing was the stored file's modification time, which is a proxy that drifts: any tool touching the file rewrites it, and opening a SQLite database read-only is enough to do so. Now that listings show a human-readable last-active time, that drift becomes visible and misleading, so each provider reads the timestamp its own data records — Codex from the session index, Antigravity from its conversation cache, Claude from the final transcript record.

Reading Claude's requires seeking to the end of the transcript, which file mtime would have given for free and, for an append-only file, would usually have matched. It is done anyway so that all three providers report the same kind of value; a listing where one column silently means something different for one provider is worse than the extra read.

## Consequences

The raw epoch value keeps the name `updated_at` and stays machine-shaped, and the human-readable rendering is a new sibling field, `last_active`, so a parser reading `updated_at` keeps working.

One provider's value does change. OpenCode recorded milliseconds where every other provider recorded seconds, and all of them share a single sort key, so every OpenCode session sorted above every other provider's threads in a mixed query. It is now converted to seconds on read, which makes an OpenCode `updated_at` a thousand times smaller than it used to be.
