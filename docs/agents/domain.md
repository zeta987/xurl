# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

This repo is **single-context**: one `CONTEXT.md` and one `docs/adr/` at the root. The Cargo workspace splits into `xurl-core` and `xurl-cli`, but both serve a single product and share one vocabulary, so they are not separate contexts.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root, or
- **`CONTEXT-MAP.md`** at the repo root if it exists — it points at one `CONTEXT.md` per context. Read each one relevant to the topic.
- **`docs/adr/`** — read ADRs that touch the area you're about to work in. In multi-context repos, also check `src/<context>/docs/adr/` for context-scoped decisions.

This repo also keeps design documents under `docs/` (for example `agy-provider-design.md`, `agents-uri-design.md`). `AGENTS.md` requires reading the relevant one before changing URI behaviour, query behaviour, or other user-facing semantics — treat them as required reading alongside any ADRs.

If any of these files don't exist, **proceed silently**. Don't flag their absence; don't suggest creating them upfront. The `/domain-modeling` skill (reached via `/grill-with-docs` and `/improve-codebase-architecture`) creates them lazily when terms or decisions actually get resolved.

## File structure

This repo, as it actually stands:

```
/
├── CONTEXT.md                                   ← the glossary
├── docs/
│   ├── adr/
│   │   ├── 0001-provider-native-titles-only.md
│   │   ├── 0002-real-timestamps-over-file-mtime.md
│   │   └── 0003-chrono-for-local-time.md
│   └── *-design.md                              ← per-feature design docs
├── xurl-core/
└── xurl-cli/
```

A `CONTEXT-MAP.md` at the root would mean the repo had split into several contexts, each with its own `CONTEXT.md`. That is not the case here and adding one is not planned.

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `CONTEXT.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal — either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (event-sourced orders) — but worth reopening because…_
