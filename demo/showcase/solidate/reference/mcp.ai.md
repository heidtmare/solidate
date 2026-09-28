# MCP server

Same tool set over two transports. Auth: API token; scopes + project restriction apply per call.

## Transports {#transports}

| transport | endpoint | auth | notes |
|---|---|---|---|
| streamable HTTP | `/mcp` on `solidate-server` | `Authorization: Bearer sol_…` per request | stateless; JSON responses; `Host` must be loopback or host of `SOLIDATE_PUBLIC_URL` (DNS rebinding) |
| stdio | `solidate-mcp` binary | `SOLIDATE_TOKEN` env | direct DB via `DATABASE_URL`; token validated at startup |

## Connecting Claude Code {#claude-code}

```sh
docker compose run --rm cli token create acme claude --scope write --project docs
claude mcp add --transport http solidate http://localhost:3000/mcp \
  --header "Authorization: Bearer sol_…"
```

- server `instructions` on initialize: variants model + translation workflow. No custom prompt needed.
- [[#prompts]] in Claude Code: `/mcp__solidate__<prompt>`.

## Tools {#tools}

| tool | args | returns / notes |
|---|---|---|
| `list_projects` | - | `[{slug, name, parent}]` |
| `list_docs` | `project` | entries (own + inherited, per-variant hashes), root hash |
| `project_hash` | `project` | Merkle root |
| `project_index` | `project` | `llms.txt` text |
| `search` | `query`, `project?`, `variant?`, `limit?` (20, max 100) | per-section hits: `path`, `variant`, `anchor`, `section_title`, `section_hash`, `snippet`; `anchor` null = title/path match only |
| `read_doc` | `project`, `path`, `variant?` (default `ai`, fallback `human`), `section?`, `subsections?`, `expand?` | `content`, `content_hash`, outline w/ semantic hashes; with `section`: that span's `hash` |
| `write_doc` | `project`, `path`, `variant?`, `content`, `base_hash?`, `message?`, `resolves?` | `base_hash` required unless creating; inherited path -> override; `followed_ai_hash` set when diagram edits were carried into the AI variant |
| `write_section` | `project`, `path`, `variant?`, `anchor?` \| `after?`, `subsections?`, `content`, `section_hash?`, `base_hash?`, `message?`, `resolves?` | `{content_hash, changed, anchors, sync_pending, followed_ai_hash}`; see [[#section-edits]] |
| `backlinks` | `project`, `path` | |
| `doc_history` | `project`, `path`, `variant?`, `limit?` (20) | newest first |
| `get_sync_queue` | `project` | `stale_side` (`null` = conflict), `proposal` |
| `get_untranslated` | `project` | `[{path, document_title, missing}]`; propose `missing` with no `base_hash` |
| `get_sync_item` | `project`, `path`, `anchor` | texts, base texts, diffs, `human_head`, `ai_head` |
| `resolve_sync` | `project`, `path`, `anchors` \| `paired` | |
| `get_translation_guide` | `project` | |
| `propose_translation` | `project`, `path`, `variant`, `content`, `base_hash?`, `message?`, `resolves?` | replaces open proposal for variant |
| `list_proposals` | `project` | `resolves`, `diff`, `outdated` |
| `report_sources` | `project`, `revision?`, `files` (`{path: git blob id}`), `removed?`, `replace?` | `{revision, reported_at, files}`; requires `write` |
| `get_drift_queue` | `project` | `{revision, reported_at, entries}`; see [[guide/code-drift#queue]] |
| `affected_sections` | `project`, `paths` | bound sections matching any path; no report needed |
| `context_for_paths` | `project`, `paths`, `variant?` (default `ai`) | `[{path, document_title, anchor, section_title, variant, content, hash, content_hash, paths, links}]`; section only in other variant -> that variant; `links`: `project:path#anchor`; no report needed |
| `verify_sources` | `project`, `path`, `anchors`, `revision?` | doc drift entries; `revision` != current report -> error |
| `changes_since` | `project`, `since?` (cursor \| RFC 3339) | `{cursor, documents}`; no `since` -> cursor only; see [[reference/rest-api#changes]] |

- `variant` default: `human`; except `read_doc` (`ai`, fallback `human`; response `variant` = variant read) and `search` (both variants).

## Prompts {#prompts}

Workflow templates (`prompts/list`, `prompts/get`); one user message naming the tools to call, in order. Read no data; args are strings.

| prompt | args | workflow |
|---|---|---|
| `translate-queue` | `project`, `limit?` (10) | `get_translation_guide` -> `get_sync_queue` (skip current proposals; conflicts reported, not translated) -> `get_sync_item` -> `propose_translation` (one per document) or `resolve_sync` for meaning-neutral edits -> remaining limit: `get_untranslated` -> `read_doc` -> `propose_translation` (no `base_hash`) |
| `fix-drift` | `project` | `get_drift_queue` -> `git diff <verified_revision>` -> `read_doc` -> `write_section` (human, then ai with `resolves`) -> `verify_sources` with queue `revision` |
| `document-change` | `project`, `paths` (comma/whitespace separated), `summary?` | `context_for_paths` (human) -> `write_section` human then ai (`resolves`) -> new files: `search` + insert section with `<!-- sources: -->` -> `verify_sources` |

- empty `paths` or non-integer `limit` -> `invalid_params` error.

## Editing one section {#section-edits}

| target | args | precondition |
|---|---|---|
| replace | `anchor` (+ `subsections`) | `section_hash` (span `hash` from `read_doc`, same `subsections`, no `expand`) or `base_hash`; one required |
| delete | `anchor`, `content: ""` | same as replace |
| insert | `after` (inserted after that section + its subsections) | optional |
| append | neither `anchor` nor `after` | optional; only target allowed when the variant is unwritten |

- `section_hash`: fails only if the target span changed; `base_hash`: fails on any change.
- `content` starts with the heading line. Inserted text without a leading heading -> rejected.
- rest of the document kept byte-for-byte; edit changing another section's anchor (unclosed fence, duplicate heading) -> rejected.
- response `anchors`: sections written. Keep heading text or `{#anchor}` to keep the variant pairing.
- inherited path -> override; `resolves` as in `write_doc`.

## Errors {#errors}

- app errors -> tool errors (`isError`), not protocol errors.
- stale `base_hash` -> message includes current `content_hash` + "Re-read and retry."
