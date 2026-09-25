# Variants and sync

Covers: pairing, sync states, clearing, disabling.

## How pairing works {#pairing}

- split both variants into sections; pair by anchor.
- per pair, `sync_bases` row: `(anchor, human_hash, ai_hash)` = semantic hashes at last reconciliation.
- semantic hash = BLAKE3(normalized CommonMark); marker-only edits (`*`/`-` bullets, `*`/`_` emphasis, Setext/ATX headings) do not change it; soft line breaks inside a paragraph do. Details: [[architecture/hashing]].

## Sync states {#states}

| state | human changed | ai changed | action |
|---|---|---|---|
| `in_sync` | no | no | none (also when both sides identical) |
| `human_ahead` | yes | no | translate into `ai` |
| `ai_ahead` | no | yes | translate into `human` |
| `conflict` | yes | yes | reconcile both |

- missing side = hash `None`; add/delete = change.
- human-only section -> `human_ahead`.
- missing both sides -> dropped.
- `stale_side`: `human_ahead`->`ai`; `ai_ahead`->`human`; `conflict`->`null`.

## Where you see it {#where}

- doc page: "Sync N" button; outline dot per stale section (distinct style for conflict).
- `/t/{tenant}/p/{project}/sync`: project queue.
- item view: current text both sides, text at base, diff since base.

## Clearing a stale section {#clearing}

1. write stale side with `resolves: [anchors]` (REST/MCP); web editor opened from a sync item resolves that anchor on save.
2. accept a proposal -> writes revision + reconciles `resolves`. See [[guide/agents-and-translation]].
3. resolve without edit (typo/formatting): `POST /api/v1/projects/{p}/resolve/{path}` `{"anchors": [...]}` | `{"all": true}` | `{"paired": true}`.

- auto: first write of a variant whose counterpart exists -> sections present in both reconciled.

## Turning sync off {#disable}

- per-document `sync_enabled` flag; toggle on the document sync page.
- disabled -> excluded from queue; `PutResult.sync` empty.
