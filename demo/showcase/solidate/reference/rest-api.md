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
| `GET /projects/{p}/docs/{path}` | Read a variant (`?variant=`, `?section=`, `?subsections=1`, `?expand=1`, `?format=md`) |
| `PUT /projects/{p}/docs/{path}` | Write a variant |
| `PATCH /projects/{p}/docs/{path}` | Replace, delete or insert one section |
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

`PATCH` changes a single section and leaves the rest of the variant byte for
byte. Its JSON body names the target with `anchor` (replace; empty `content`
deletes), `after` (insert after that section and its subsections) or neither
(append), plus `content`, `subsections`, `message` and `resolves`. For a
replacement, send the section's `hash` from `GET ?section=` as `section_hash`,
and the write fails with `412` only if that section changed. An `If-Match`
header works too, but fails on a change anywhere in the variant. Replacing
without either is refused with `428`. An edit that would shift the anchor of
another section, such as an unclosed code fence, is refused with `400`. The
response adds `anchors`, the sections written. [[reference/mcp#section-edits]]
covers the same operation for agents.

## Projects, search and indexes {#projects}

| Method and path | Purpose |
|---|---|
| `GET /projects` | Projects visible to the token, with parents |
| `GET /projects/{p}` | One project |
| `GET /projects/{p}/tree` | Effective documents, own and inherited, with hashes |
| `GET /projects/{p}/hash` | Merkle root over the tree; supports `If-None-Match` |
| `GET /search?q=&project=&variant=&limit=` | Full-text search; one hit per matching section, with its anchor, hash and a highlighted snippet |
| `GET /projects/{p}/llms.txt` | Plain-text index for language models |

## Change feed {#changes}

`GET /projects/{p}/changes?since=` lists the documents that changed in the
project, including inherited ones, since `since`. That is either the `cursor`
from a previous response or an RFC 3339 timestamp. Without `since` the response
holds only a cursor, which a client stores to start from. Scheduled agents and
bots use it to pick up where they stopped:

```sh
curl -H "Authorization: Bearer $SOLIDATE_TOKEN" \
  "http://localhost:3000/api/v1/projects/solidate/changes?since=81234"
```

Each document entry says whether it was created or deleted in the window and,
per variant, lists the section anchors added, changed and removed between the
revision before the window and the last one in it, along with the authors and
revision messages. Sections are compared by semantic hash, so formatting-only
edits show up with empty lists. Writing an override of an inherited document
counts as creating it, and the inherited entry is left out.

The cursor is a database transaction bound. A change appears once every
transaction that started before it has finished, so a slow commit is never
skipped, but a long-running transaction delays the feed until it ends.

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

## Source drift {#drift}

| Method and path | Purpose |
|---|---|
| `POST /projects/{p}/sources` | Report repository file hashes |
| `GET /projects/{p}/drift` | Bound sections whose sources drifted |
| `GET /projects/{p}/drift/{path}` | Drift status of every bound section of a document |
| `POST /projects/{p}/affected` | Sections that describe the given paths |
| `POST /projects/{p}/context` | The same sections with their content |
| `POST /projects/{p}/verify/{path}` | Confirm sections still match their sources |

See [[guide/code-drift]] for the workflow.

## Audit {#audit}

`GET /audit` lists the tenant's audit log, newest first. It requires the `admin`
scope. Page with `limit` (default 100, at most 500) and `before`, using the `next` value from the previous page.
