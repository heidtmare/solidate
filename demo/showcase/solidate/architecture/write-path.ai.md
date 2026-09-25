# Write path

All writes (web, REST, MCP, CLI import, proposal accept) -> `App::put_doc` / `put_doc_tx`. One tenant transaction.

## Preconditions {#preconditions}

| source | expectation |
|---|---|
| web editor | hidden `base` = content hash read |
| MCP `write_doc` | `base_hash`; omitted = create |
| REST `If-Match: "<h>"` | `Expect::Head(h)` |
| REST `If-Match: *` | `Expect::Any` |
| REST `If-None-Match: *` | `Expect::Absent` |
| REST none | 428 `precondition_required` |
| CLI import | `Expect::Any` |

## Checks {#checks}

1. `content.len() <= max_doc_bytes` (1 MiB) else 400.
2. `ctx.require(Write, project)` else 403.
3. path not own document:
   - inherited and `Expect::Head(h)` -> `h` must equal inherited head hash else 412.
   - create document in this project (override); expectation becomes `Absent`.

## Committing the revision {#commit}

1. `analyze(content)` -> sections, anchors, links, content + semantic hashes.
2. `SELECT revision_id, content_hash FROM heads ... FOR UPDATE`.
3. expectation mismatch -> 412 with current hash.
4. content hash == head -> no-op; `changed: false`.
5. insert blob (`ON CONFLICT DO NOTHING`), revision (`parent_id` = old head), move head (title, tsvector), sections, links.

## After the write {#after}

- changed -> delete open proposal for this variant.
- `sync_enabled`:
  - recompute plan; reconcile `resolves`.
  - first write of variant with existing counterpart -> also reconcile sections present in both.
  - prune `sync_bases` for anchors absent from plan.
- audit `doc.write` `{variant, revision, content_hash, changed, resolves}` when changed or `resolves` non-empty.
- single transaction: revision + sync + audit atomic.

## Response {#response}

- `{path, variant, content_hash, revision, changed, sync: [{anchor, state}]}` (sync = attention-needing only).
- `ETag` = new content hash; 201 on create, else 200.
