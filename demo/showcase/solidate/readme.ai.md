# Solidate

Wiki-style knowledge base for project design docs; read/written by humans and AI agents. Each document = one ground truth, two variants: `human` (narrative), `ai` (dense reference). Sync tracked per section. This project documents Solidate and is stored in Solidate.

## Why two variants {#why}

- human docs: context, trade-offs. agents: exact names, paths, limits.
- one combined doc serves neither; two unlinked docs drift.
- Solidate: sections paired by anchor, hashed; edit one side -> other side stale until translated or marked in sync.
- rationale: [[decisions/0002-dual-variants]].

## What you get {#features}

| feature | detail |
|---|---|
| UI | server-rendered HTML (Topcoat); htmx only; no JS framework |
| sync | per-section queue; diffs; resolve without edit |
| agent translation | proposals; human accept/reject |
| inheritance | child projects inherit docs (guide, rules); override by path |
| references | `{{include}}`, wiki links, backlinks |
| hashing | BLAKE3 content hash = ETag; semantic hash for sync; Merkle root per project |
| APIs | REST `/api/v1`, MCP (`/mcp`, stdio), `llms.txt` |
| tenancy | PostgreSQL RLS |

## Vocabulary {#vocabulary}

Source: parent project `handbook`.

{{include handbook:glossary#core-terms}}

## Where to go next {#next}

- start: [[guide/quickstart]], [[guide/writing-documents]]
- sync: [[guide/variants-and-sync]], [[guide/agents-and-translation]]
- ops: [[guide/administration]], [[reference/configuration]]
- integration: [[reference/rest-api]], [[reference/mcp]]
- internals: [[architecture/overview]]
