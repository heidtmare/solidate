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
| GET | `/projects/{p}/docs/{path}` | `variant=human\|ai`, `section=<anchor>`, `expand=1`, `format=md\|json` (or `Accept: text/markdown`) |
| PUT | `/projects/{p}/docs/{path}` | body `text/markdown` (+ `message`, `resolves=a,b` query) or JSON `{content, message?, resolves?}` |
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

- PUT response: `{path, variant, content_hash, revision, changed, sync: [{anchor, state}]}`.
- max body: 1 MiB per variant. `\r\n` normalized to `\n`.

## Projects, search and indexes {#projects}

| method | path | notes |
|---|---|---|
| GET | `/projects` | `[{slug, name, parent}]` |
| GET | `/projects/{p}` | |
| GET | `/projects/{p}/tree` | own + inherited; `ETag` = root hash |
| GET | `/projects/{p}/hash` | `{root_hash}`; `If-None-Match` -> 304 |
| GET | `/search` | `q` (websearch syntax), `project`, `limit` (default 20) |
| GET | `/projects/{p}/llms.txt` | `text/plain`; links AI variant raw Markdown (human if no AI) |

## Sync and proposals {#sync}

| method | path | notes |
|---|---|---|
| GET | `/projects/{p}/sync` | queue |
| GET | `/projects/{p}/sync/{path}` | doc status; `anchor=` -> sync item |
| POST | `/projects/{p}/resolve/{path}` | exactly one of `{"anchors": [...]}`, `{"all": true}`, `{"paired": true}` |
| GET | `/projects/{p}/translation-guide` | `{source, content}` |
| GET | `/projects/{p}/proposals` | with `diff`, `outdated` |
| POST | `/projects/{p}/propose/{path}` | `{variant, content, base_hash?, message?, resolves?}`; 201 |
| DELETE | `/projects/{p}/proposals/{id}` | reject; 204 |

- accept: web UI only (human decision). See [[guide/agents-and-translation]].

## Audit {#audit}

- `GET /audit`: newest first; scope `admin`; `limit` (default 100, max 500), `before=<id>`; response includes `next` cursor.
