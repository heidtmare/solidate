# MCP server

Same tool set over two transports. Auth: API token; scopes + project restriction apply per call.

## Transports {#transports}

| transport | endpoint | auth | notes |
|---|---|---|---|
| streamable HTTP | `/mcp` on `solidate-server` | `Authorization: Bearer sol_…` per request | stateless; JSON responses; `Host` must be loopback or host of `SOLIDATE_PUBLIC_URL` (DNS rebinding) |
| stdio | `solidate-mcp` binary | `SOLIDATE_TOKEN` env | direct DB via `DATABASE_URL`; token validated at startup |

## Connecting Claude Code {#claude-code}

```sh
docker compose run --rm cli token create acme claude --scope write --project docs
claude mcp add --transport http solidate http://localhost:3000/mcp \
  --header "Authorization: Bearer sol_…"
```

- server `instructions` on initialize: variants model + translation workflow. No custom prompt needed.

## Tools {#tools}

| tool | args | returns / notes |
|---|---|---|
| `list_projects` | - | `[{slug, name, parent}]` |
| `list_docs` | `project` | entries (own + inherited, per-variant hashes), root hash |
| `project_hash` | `project` | Merkle root |
| `project_index` | `project` | `llms.txt` text |
| `search` | `query`, `project?`, `limit?` (20, max 100) | hits with snippets |
| `read_doc` | `project`, `path`, `variant?`, `section?`, `expand?` | `content`, `content_hash`, outline w/ semantic hashes |
| `write_doc` | `project`, `path`, `variant?`, `content`, `base_hash?`, `message?`, `resolves?` | `base_hash` required unless creating; inherited path -> override |
| `backlinks` | `project`, `path` | |
| `doc_history` | `project`, `path`, `variant?`, `limit?` (20) | newest first |
| `get_sync_queue` | `project` | `stale_side` (`null` = conflict), `proposal` |
| `get_sync_item` | `project`, `path`, `anchor` | texts, base texts, diffs, `human_head`, `ai_head` |
| `resolve_sync` | `project`, `path`, `anchors` \| `paired` | |
| `get_translation_guide` | `project` | |
| `propose_translation` | `project`, `path`, `variant`, `content`, `base_hash?`, `message?`, `resolves?` | replaces open proposal for variant |
| `list_proposals` | `project` | `resolves`, `diff`, `outdated` |

- `variant` default: `human`.

## Errors {#errors}

- app errors -> tool errors (`isError`), not protocol errors.
- stale `base_hash` -> message includes current `content_hash` + "Re-read and retry."
