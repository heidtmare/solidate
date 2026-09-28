# REST API

Base: `/api/v1`. Auth: bearer API token; token determines tenant (no tenant in URLs). JSON unless noted.

## Authentication and errors {#auth}

```sh
curl -H "Authorization: Bearer $SOLIDATE_TOKEN" http://localhost:3000/api/v1
```

- `GET /api/v1` -> `{tenant, tenant_name, scopes, project}`.
- error body: `{"error": {"code", "message"}}`.

```json
{"error": {"code": "precondition_failed", "message": "the document changed; ..."}}
```

| status | code |
|---|---|
| 400 | `bad_request` |
| 401 | `unauthorized` (+ `WWW-Authenticate: Bearer`) |
| 403 | `forbidden` |
| 404 | `not_found` |
| 409 | `already_exists` |
| 412 | `precondition_failed` (+ current `ETag`) |
| 428 | `precondition_required` |
| 429 | `rate_limited` (+ `Retry-After` seconds) |
| 500 | `internal` |

## Concurrency and caching {#etags}

- `ETag`: strong, `"<content hash hex>"`; with `expand=1` it is the resolved hash.
- read: `If-None-Match` -> 304.
- write precondition (exactly one required):
  - `If-Match: "<hash>"` -> expect that head.
  - `If-Match: *` -> any.
  - `If-None-Match: *` -> create only; 201.
  - none -> 428; mismatch -> 412.
- flow: [[architecture/write-path]].

## Documents {#documents}

| method | path | notes |
|---|---|---|
| GET | `/projects/{p}/docs/{path}` | `variant=human\|ai`, `section=<anchor>`, `subsections=1`, `expand=1`, `format=md\|json` (or `Accept: text/markdown`) |
| PUT | `/projects/{p}/docs/{path}` | body `text/markdown` (+ `message`, `resolves=a,b` query) or JSON `{content, message?, resolves?}` |
| PATCH | `/projects/{p}/docs/{path}` | one section; JSON `{anchor? \| after?, subsections?, content, section_hash?, message?, resolves?}`; `variant` query |
| DELETE | `/projects/{p}/docs/{path}` | both variants; owned docs only; 204 |
| GET | `/projects/{p}/history/{path}` | `variant`; newest first |
| GET | `/projects/{p}/revisions/{rev}/{path}` | content + diff from parent |
| GET | `/projects/{p}/backlinks/{path}` | |

```sh
curl -X PUT "http://localhost:3000/api/v1/projects/solidate/docs/guide/quickstart?variant=ai&resolves=start" \
  -H "Authorization: Bearer $SOLIDATE_TOKEN" \
  -H 'If-Match: "5f3c…"' -H "Content-Type: text/markdown" \
  --data-binary @quickstart.ai.md
```

- PUT response: `{path, variant, content_hash, revision, changed, sync: [{anchor, state}], followed_ai_hash?}`; `followed_ai_hash` = new AI `content_hash` when diagram edits were carried over ([[guide/variants-and-sync#diagrams]]).
- PATCH targets: `anchor` -> replace (empty `content` deletes); `after` -> insert after section + subsections; neither -> append.
- PATCH precondition: `section_hash` (the `hash` from `GET ?section=`, same `subsections`, no `expand`) -> 412 only if that span changed; or `If-Match` (whole variant). Replace without either -> 428; inserts need none.
- PATCH rejects (400): another section's anchor would change (unclosed fence, duplicate heading); inserted text without leading heading.
- PATCH response: PUT response + `anchors` (sections written). Agent equivalent: [[reference/mcp#section-edits]].
- max body: 1 MiB per variant. `\r\n` normalized to `\n`.

## Projects, search and indexes {#projects}

| method | path | notes |
|---|---|---|
| GET | `/projects` | `[{slug, name, parent}]` |
| GET | `/projects/{p}` | |
| GET | `/projects/{p}/tree` | own + inherited; `ETag` = root hash |
| GET | `/projects/{p}/hash` | `{root_hash}`; `If-None-Match` -> 304 |
| GET | `/search` | `q` (websearch syntax), `project`, `variant` (both if absent), `limit` (default 20) -> `[{project, path, variant, title, anchor, section_title, section_hash, rank, snippet}]`; one hit per matching section; `anchor` null = title/path match only |
| GET | `/projects/{p}/llms.txt` | `text/plain`; links AI variant raw Markdown (human if no AI) |

## Change feed {#changes}

- `GET /projects/{p}/changes?since=<cursor|RFC 3339>` -> `{cursor, documents: [{path, title, owner, inherited, created, deleted, variants: [{variant, revisions, content_hash, added, changed, removed, authors: [{kind, name}], messages, updated_at}]}]}`; ordered by path.
- no `since` -> `{cursor, documents: []}`. Invalid `since` -> 400.
- covers own + inherited docs; override written in window -> `created`, inherited entry omitted. `deleted` -> `variants: []`.
- anchors: semantic-hash diff of revision before window vs last in window; formatting-only -> empty lists.
- cursor = `pg_snapshot_xmin` bound: no change skipped on late commit; long-running transactions delay visibility. Next call: pass response `cursor`.

## Sync and proposals {#sync}

| method | path | notes |
|---|---|---|
| GET | `/projects/{p}/sync` | queue |
| GET | `/projects/{p}/sync/{path}` | doc status; `anchor=` -> sync item |
| GET | `/projects/{p}/untranslated` | `[{path, document_title, missing}]`; sync-enabled own docs with one variant |
| POST | `/projects/{p}/resolve/{path}` | exactly one of `{"anchors": [...]}`, `{"all": true}`, `{"paired": true}` |
| GET | `/projects/{p}/translation-guide` | `{source, content}` |
| GET | `/projects/{p}/proposals` | with `diff`, `outdated` |
| POST | `/projects/{p}/propose/{path}` | `{variant, content, base_hash?, message?, resolves?}`; 201 |
| DELETE | `/projects/{p}/proposals/{id}` | reject; 204 |

- accept: web UI only (human decision). See [[guide/agents-and-translation]].

## Source drift {#drift}

| method | path | notes |
|---|---|---|
| POST | `/projects/{p}/sources` | `{revision?, files: {path: hash}, removed?, replace?}` -> `{revision, reported_at, files}`; scope `write` |
| GET | `/projects/{p}/drift` | `{revision, reported_at, entries}`; entries not `fresh` or with `missing` |
| GET | `/projects/{p}/drift/{path}` | all bound sections of the doc, incl. `fresh` |
| POST | `/projects/{p}/affected` | `{paths}` -> `[{path, anchor, section_title, paths}]` |
| POST | `/projects/{p}/context` | `{paths, variant?}` (default `ai`) -> `[{path, document_title, anchor, section_title, variant, content, hash, content_hash, paths, links}]` |
| POST | `/projects/{p}/verify/{path}` | `{anchors, revision?}` -> doc drift entries; scope `write` |

- entry fields and states: [[guide/code-drift#queue]].

## Audit {#audit}

- `GET /audit`: newest first; scope `admin`; `limit` (default 100, max 500), `before=<id>`; response includes `next` cursor.
