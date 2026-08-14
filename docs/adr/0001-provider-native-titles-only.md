# Titles come only from provider-native storage

Query listings show a thread's title so a human can recognise it, and that title is only ever the one the provider itself recorded — when a provider stored none, the field is simply absent. Deriving a title from the first user message was considered and rejected: in practice that first message is boilerplate (Codex threads routinely open with harness text, and agent-driven sessions open with a system prompt), so a derived title would look authoritative while identifying nothing.

Where each provider keeps its title is worth recording, because two of the three are not where a reader would look first:

- **Codex** keeps titles in `~/.codex/session_index.jsonl`, a separate index file — one line per thread carrying `id`, `thread_name`, and `updated_at`. The session transcripts under `sessions/` contain no title at all, so reading them is wasted work. The index covered 243 of 249 local threads, all 243 with a title.
- **Claude Code** writes `{"type":"custom-title","customTitle":"…"}` records inside the transcript itself, present in roughly 40% of threads (it reflects a title the user set) and located early enough to fall inside the existing line budget.
- **Antigravity** carries it in the protobuf step payload, already extracted during materialisation.

## Consequences

Coverage is deliberately partial — about 60% of Claude threads and a handful of the newest Codex threads will list without a title. That is accepted: those threads are still identified by their working directory, branch, and last-active time, and an absent title is honest where a fabricated one would mislead.
