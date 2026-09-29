# Variants and sync

Covers: pairing, sync states, single-variant documents, diagrams, clearing, restoring, undeleting, disabling.

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

## Restoring an earlier version {#restore}

- restore = new revision with the old content; `restored_from` = source revision; heads never move back. Change feed, audit (`doc.write` + `doc.restore`), `If-Match` as for any write.
- entry points: web history "Restore" / revision page "Restore this version" -> dry-run preview -> confirm | `POST /api/v1/projects/{p}/restore/{path}` | MCP `restore_revision`. Owning project only.
- other variant, per section changed by the restore:

| situation | result |
|---|---|
| other side not yet translated (restored hash = base) | `in_sync`, no write |
| other side unchanged since last sync, pairing recorded | companion restore: other side set to text last in sync with restored text; removed section removed; missing section inserted after nearest shared predecessor |
| other side changed since last sync | `skipped`; stays in queue |
| no pairing recorded for restored text | `skipped` |

- companion default on; `companion: false` disables. Companion write message "Restore sections paired with {variant} revision {hash}"; no diagram follow.
- base restoration: after both writes, affected section whose current `(human, ai)` hashes were ever recorded as a base -> that base recorded again (`reconciled`). Covers reverting a bad write that was marked in sync with `resolves`.
- source of pairings: `sync_base_log` (append-only; see [[architecture/data-model#sync]]).
- `SyncItem.paired`: stale side's text last recorded in sync with the other side's current text; present after the other side went back to an earlier version. Write it back instead of retranslating.

### Restoring a deleted document {#undelete}

- delete = soft delete; revisions, heads, `sync_bases`, proposals kept. `deleted_by_user` | `deleted_by_token` recorded.
- entry points: web project "Deleted" page (list, read-only view, "Restore"); "Undo" notice after a delete | `GET /api/v1/projects/{p}/deleted`, `GET .../deleted/{id}`, `POST .../deleted/{id}/restore`. Owning project only; `write` scope. No MCP tool (MCP cannot delete either).
- undelete by document id: a path can have several deleted documents.
- live document at the path -> `already_exists` (409); no restore under another path.
- result: document live with its last heads and sync state; next write's parent = last head. Audit `doc.undelete` `{document, deleted_at}`; `doc.delete` detail carries `document`.
- change feed: `restored: true`; deleted and restored in one window -> `restored` only, `deleted: false`. `variants` covers only revisions in the window; read the document.

## Turning sync off {#disable}

- per-document `sync_enabled` flag; toggle on the document sync page.
- disabled -> excluded from queue; `PutResult.sync` empty.
