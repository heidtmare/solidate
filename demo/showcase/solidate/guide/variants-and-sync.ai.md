# Variants and sync

Covers: pairing, sync states, single-variant documents, diagrams, clearing, disabling.

## How pairing works {#pairing}

- split both variants into sections; pair by anchor.
- per pair, `sync_bases` row: `(anchor, human_hash, ai_hash)` = sync hashes at last reconciliation.
- sync hash = BLAKE3(normalized CommonMark, diagram fence contents blanked); marker-only edits (`*`/`-` bullets, `*`/`_` emphasis, Setext/ATX headings) do not change it; soft line breaks inside a paragraph do. Details: [[architecture/hashing]], [[#diagrams]].

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
- `needs_person`: `ai_ahead`, `conflict`. `human_ahead` -> agents.

## Documents with one variant {#single}

- one variant written -> no pairs; not in queue; `PutResult.sync` empty.
- sync starts on first write of the second variant (shared sections auto-reconciled, see [[#clearing]]).
- discovery: `GET /api/v1/projects/{p}/untranslated` | MCP `get_untranslated` | CLI `solidate untranslated` -> `[{path, document_title, missing}]`; sync-enabled own documents only.

## Diagrams {#diagrams}

- diagram fence = fenced code block, language in `DIAGRAM_LANGUAGES` (`mermaid`).
- shared by both variants, not translated; sync hash keeps fence + info string, drops contents.
- edit contents -> no staleness. add/remove fence -> change.
- auto-follow: human-variant write where a diagram changed (top-level, closed, same position in a section with unchanged diagram count) and the AI section with the same anchor holds exactly one identical copy of the old contents -> AI revision written in the same transaction (message "Carry over diagram changes from the human variant"; audit `doc.write` with `follows: human`). Response `followed_ai_hash` = new AI `content_hash`.
- not followed: AI copy diverged, missing, ambiguous; sync disabled; AI-variant edits. `PutResult.diagrams_carried` / `diagrams_skipped` = anchors carried / skipped (skipped: AI section has diagrams but no single identical copy); web editor redirect adds `?carried=` / `?skipped=` and the doc page shows a notice.
- drift: diagrams at the same position in same-anchor sections with equal diagram counts and different contents. Doc sync status `diagram_drift` (`anchor`, `index`, `lang`, `human`, `ai`); project list via `App::diagram_drift`. Not a sync state.
- resolve drift: `POST /t/{tenant}/p/{project}/copy-diagram/{path}` (`anchor`, `index`, `from`) writes the other variant with `from`'s diagram contents (message "Copy diagram from the {from} variant"; audit `doc.write`, `follows: from`). Sync unchanged. Agents do not resolve drift.

## Where you see it {#where}

- doc page: "Sync N" button, N = `needs_person` sections; outline dot per stale section (conflict: distinct; `human_ahead`: hollow).
- `/t/{tenant}/p/{project}/sync`: project queue, grouped "Needs a person" / "Waiting on agents".
- item view: current text both sides, text at base, diff since base.

## Clearing a stale section {#clearing}

1. write stale side with `resolves: [anchors]` (REST/MCP); web editor opened from a sync item resolves that anchor on save.
2. accept a proposal -> writes revision + reconciles `resolves`. See [[guide/agents-and-translation]].
3. resolve without edit (typo/formatting): `POST /api/v1/projects/{p}/resolve/{path}` `{"anchors": [...]}` | `{"all": true}` | `{"paired": true}`.

- auto: first write of a variant whose counterpart exists -> sections present in both reconciled.

## Turning sync off {#disable}

- per-document `sync_enabled` flag; toggle on the document sync page.
- disabled -> excluded from queue; `PutResult.sync` empty.
