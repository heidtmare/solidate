# Agents and translation

Agents authenticate with API tokens (REST or MCP). Revisions attributed to the token. Covers expected agent workflow and human control.

## Two kinds of agent work {#kinds}

- authoring (agent changes meaning): write edited variant, then write other variant with `resolves: [translated anchors]` -> pair never stale.
- translating others' changes: MUST use proposals (human review).

## The translation guide {#guide}

- path: `_meta/translation`; resolved through inheritance (nearest project wins).
- this project: inherited from `handbook`.
- none in chain: built-in default (`DEFAULT_GUIDE`).
- REST `GET /api/v1/projects/{p}/translation-guide` -> `{source, content}`; MCP `get_translation_guide`.

## The translation loop {#loop}

1. read guide (once per project).
2. queue: `GET /api/v1/projects/{p}/sync` | MCP `get_sync_queue`; skip entries with non-outdated `proposal`.
3. item: `GET /api/v1/projects/{p}/sync/{path}?anchor={a}` | MCP `get_sync_item` -> both texts, base texts, diffs, `human_head`/`ai_head` hashes.
4. submit: `POST /api/v1/projects/{p}/propose/{path}` `{variant, content, base_hash?, message?, resolves?}` | MCP `propose_translation`.

- one proposal per (document, variant); resubmission replaces -> retry-safe.
- `resolves` empty -> all attention-needing sections that remain in the plan.
- rejected if other variant never written (`nothing to translate`) or `base_hash` != current (412).

## Reviewing proposals {#review}

- listed on project Sync page with diff vs current variant.
- accept: writes revision (author = reviewer), reconciles `resolves`, deletes proposal; audit `proposal.accept` with `proposer`.
- reject: deletes; audit `proposal.reject`. REST `DELETE /api/v1/projects/{p}/proposals/{id}`.
- outdated: either head moved since submission (`base_hash` or `source_hash` mismatch) -> accept fails 412; resubmit.

## Guardrails {#guardrails}

- scopes: `read` < `write` < `admin` (hierarchical); optional project restriction; optional expiry.
- writes require base hash (optimistic concurrency).
- rate limit per token (default 600/min, burst 60).
- audit log entry per mutation with `actor_token_id`.
- see [[architecture/security]], [[reference/mcp]].
