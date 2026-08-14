# xURL

xURL reads, queries, and writes AI agent conversations through one `agents://` URI scheme. This glossary fixes the vocabulary shared across every provider, so the same concept keeps the same name whether it appears in a URI, a field name, or the README.

## Language

**Provider**:
One AI coding agent CLI whose stored conversations xurl can reach, such as `codex`, `claude`, or `agy`.
_Avoid_: backend, adapter, integration, source

**Thread**:
One conversation with one provider, addressed as `agents://<provider>/<id>`. This is the word for field names, URI syntax, and code.
_Avoid_: session, chat

**Conversation**:
The same thing as a Thread, but the word to use in user-facing prose. `README.md` and `skills/xurl/SKILL.md` say conversation; `thread_id` and `agents://codex/<id>` stay Thread.
_Avoid_: mixing the two within one document

**Title**:
A thread's human-readable name, taken from whatever the provider itself recorded. A thread has no title when its provider stored none — xurl never invents one from message content.
_Avoid_: name, summary, subject, label

**Last active**:
When a thread was last worked on, expressed for a human reader. Distinct from the raw epoch value, which keeps the name `updated_at`.
_Avoid_: updated, modified, last modified

**Role**:
A named entry point under a provider that starts or finds work by function rather than by id, as in `agents://codex/reviewer`.
_Avoid_: agent, persona, mode

**Child**:
A subagent thread nested under a parent thread, reached by appending its id — `agents://<provider>/<thread>/<child>`.
_Avoid_: sub-thread, nested session

**Path-scoped query**:
A query that selects threads by the local working directory they ran in rather than by provider, written `agents:///abs/path`. Always returns a list, never a single thread.
_Avoid_: directory search, folder query

**Discovery**:
Reading a thread's frontmatter and child targets without its content, via `-I`. Serves agents that need complete, machine-shaped data.
_Avoid_: preview, peek, head
