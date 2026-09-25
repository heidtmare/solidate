# REST API

The REST API lives under `/api/v1`. It exposes everything an integration needs:
reading and writing documents, the sync queue, proposals, search and history.
Requests authenticate with an API token, and the token alone determines the
tenant, so tenant slugs never appear in API URLs.

## Authentication and errors {#auth}

Send the token as a bearer credential:

```sh
curl -H "Authorization: Bearer $SOLIDATE_TOKEN" http://localhost:3000/api/v1
```

`GET /api/v1` answers with the tenant, the token's scopes and, for restricted
tokens, the project it is bound to. Errors always share one shape:

```json
{"error": {"code": "precondition_failed", "message": "the document changed; ..."}}
```

Requests are rate limited per token. A token that exhausts its budget receives
`429 rate_limited` with a `Retry-After` header in seconds.

## Concurrency and caching {#etags}

Every document variant has a content hash, returned as a strong `ETag`. Reads
honour `If-None-Match` and answer `304 Not Modified` when nothing changed. Writes
must say what they expect to replace: `If-Match: "<hash>"` for the version you
read, `If-Match: *` to overwrite whatever is there, or `If-None-Match: *` to
create a variant that must not exist yet. A write without any of these is refused
with `428`, and one whose expectation is wrong gets `412` with the current ETag,
so the client can re-read and retry. [[architecture/write-path]] shows the full
decision flow.

## Documents {#documents}

| Method and path | Purpose |
|---|---|
| `GET /projects/{p}/docs/{path}` | Read a variant (`?variant=`, `?section=`, `?expand=1`, `?format=md`) |
| `PUT /projects/{p}/docs/{path}` | Write a variant |
| `DELETE /projects/{p}/docs/{path}` | Delete both variants of an owned document |
| `GET /projects/{p}/history/{path}` | Revisions of a variant, newest first |
| `GET /projects/{p}/revisions/{rev}/{path}` | One revision and its diff from the parent |
| `GET /projects/{p}/backlinks/{path}` | Documents linking here |

A write takes either raw Markdown (`Content-Type: text/markdown`, with `message`
and `resolves` as query parameters) or JSON `{content, message, resolves}`:

```sh
curl -X PUT "http://localhost:3000/api/v1/projects/solidate/docs/guide/quickstart?variant=ai&resolves=start" \
  -H "Authorization: Bearer $SOLIDATE_TOKEN" \
  -H 'If-Match: "5f3c…"' -H "Content-Type: text/markdown" \
  --data-binary @quickstart.ai.md
```

The response reports the new content hash, whether anything changed, and which
sections of the document still need sync.

## Projects, search and indexes {#projects}

| Method and path | Purpose |
|---|---|
| `GET /projects` | Projects visible to the token, with parents |
| `GET /projects/{p}` | One project |
| `GET /projects/{p}/tree` | Effective documents, own and inherited, with hashes |
| `GET /projects/{p}/hash` | Merkle root over the tree; supports `If-None-Match` |
| `GET /search?q=&project=&limit=` | Full-text search with highlighted snippets |
| `GET /projects/{p}/llms.txt` | Plain-text index for language models |

## Sync and proposals {#sync}

| Method and path | Purpose |
|---|---|
| `GET /projects/{p}/sync` | Stale sections across the project |
| `GET /projects/{p}/sync/{path}` | Per-section status; `?anchor=` for the full sync item |
| `POST /projects/{p}/resolve/{path}` | Mark sections in sync without editing |
| `GET /projects/{p}/translation-guide` | The guide in effect |
| `GET /projects/{p}/proposals` | Open proposals |
| `POST /projects/{p}/propose/{path}` | Submit a translation proposal |
| `DELETE /projects/{p}/proposals/{id}` | Withdraw a proposal |

Accepting a proposal is deliberately a web-only action, so a person makes that
call. See [[guide/agents-and-translation]].

## Audit {#audit}

`GET /audit` lists the tenant's audit log, newest first. It requires the `admin`
scope. Page with `limit` (default 100, at most 500) and `before`, using the `next` value from the previous page.
