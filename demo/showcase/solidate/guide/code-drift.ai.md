# Keeping docs in step with code

Source bindings: a section declares the repository files it describes; clients report file hashes; Solidate lists sections whose files changed since last verification. Complements human/AI sync (variant↔variant drift) with doc↔code drift.

## Declaring sources {#bindings}

<!-- sources: crates/solidate-core/src/sources.rs, crates/solidate-core/src/markdown.rs -->

```md
## Token validation {#tokens}

<!-- sources: crates/solidate-app/src/auth.rs, crates/solidate-db/migrations/ -->
```

- form: top-level HTML block `<!-- sources: p1, p2 -->`; separators `,` or newline.
- belongs to the enclosing top-level section; before first heading -> `_preamble`.
- document bindings = union of both variants, per anchor.
- patterns: relative to repo root; `*`, `?` within a segment; `**` any segments; trailing `/` = `dir/**`.
- rejected on write (400 / tool error): empty, absolute, `.`/`..` segments, empty segments, backslash, control chars, >1024 bytes.
- not rendered; excluded from semantic hashes -> adding/editing never marks the other variant stale; directive-only preamble is not a section.

## Reporting the repository {#reporting}

<!-- sources: crates/solidate-app/src/sources.rs, crates/solidate-cli/src/main.rs -->

- Solidate never reads the repo; clients report `path -> hash` per project (typically CI on main).
- hash: opaque, 1-128 printable ASCII; convention: git blob id (`git ls-files -s`) so reporters agree.
- CLI: `solidate sources report <tenant> <project> [--dir .] [--revision <rev>]` -> `git ls-files -s -z` (index), revision default `git rev-parse HEAD`, `replace: true`.
- REST `POST /api/v1/projects/{p}/sources` `{revision?, files: {path: hash}, removed?: [path], replace?: bool}`; MCP `report_sources`.
- `replace: true`: `files` = complete tree; `removed` not allowed. Otherwise upsert `files`, delete `removed`.
- limits: 200000 files per report; revision 1-200 chars, no control chars. Requires `write`.

## The drift queue {#queue}

<!-- sources: crates/solidate-app/src/sources.rs -->

| state | meaning |
|---|---|
| `unverified` | no verification record |
| `changed` | matched files differ from record (`changed`, `added`, `removed` paths) |
| `fresh` | matched files == record |

- queue entry: `path`, `anchor`, `section_title`, `state`, `patterns`, `missing` (patterns matching no reported file), `changed`, `added`, `removed`, `verified_revision`, `verified_at`.
- listed when `state != fresh` or `missing` non-empty.
- scope: project's own documents (not inherited), including sync-disabled ones.
- no report yet: queue has `revision: null`, `reported_at: null`; all patterns missing.
- catch-up diff: `git diff <verified_revision> -- <changed paths>`.

## Working through drift {#workflow}

1. queue: MCP `get_drift_queue` | `GET /api/v1/projects/{p}/drift`.
2. per entry: update section (then translate via sync queue as usual) or confirm still accurate.
3. verify: MCP `verify_sources {project, path, anchors, revision?}` | `POST /api/v1/projects/{p}/verify/{path}` -> records currently matched files; audit `sources.verify`.

- after code changes: MCP `affected_sections {project, paths}` | `POST /api/v1/projects/{p}/affected` `{paths}` -> bound sections matching any path; needs no report.
- `revision` on verify: must equal current report revision, else error (newer report arrived).
- verify errors: no report for project; anchor without bindings; empty `anchors`.
- bindings removed from both variants -> verification record pruned on write.
- this page binds its own sections.
