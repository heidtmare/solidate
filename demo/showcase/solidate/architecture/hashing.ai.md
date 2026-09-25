# Hashing

Algorithm: BLAKE3, 256-bit, hex-encoded (64 chars). Two kinds + two aggregates.

## Content hashes {#content}

- input: exact bytes of a variant.
- uses: `blobs` key (dedup), `ETag`, write precondition (`If-Match` / `base_hash`).
- any byte change -> new hash.

## Semantic hashes {#semantic}

- input: `markdown_to_commonmark(section, options)` (normalized).
- formatting-only edits (Setext vs ATX, `*` vs `_`, list markers) -> same hash.
- soft line breaks preserved by normalization: rewrapping a paragraph -> new hash.
- used by sync (`sections.hash`, `sync_bases`). Document-level `revisions.semantic_hash` also stored.

## Merkle roots {#merkle}

- encoding: entries sorted by key; each `key \0 hex \n`; BLAKE3 over concatenation.
- order-independent; keys must not contain `\0` or `\n`.
- project root: keys `path@variant` over effective docs (own + inherited).
- `GET /api/v1/projects/{p}/hash` + `If-None-Match` -> 304 when unchanged; MCP `project_hash`.

## Resolved hashes {#resolved}

- documents with includes: Merkle(`"" -> content hash`, `"{i}:{target}" -> piece hash`).
- missing include -> hash of empty string.
- no includes -> equals content hash.
- exposed with `expand=1` as `resolved_hash` and `ETag`.
- example: `readme` includes `handbook:glossary#core-terms`.
