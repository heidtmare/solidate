# MCP server

Solidate speaks the Model Context Protocol, so coding agents such as Claude Code
can read and maintain documentation directly. The same set of tools is available
over two transports, and in both cases the agent acts with an API token whose
scopes and project restriction apply to every call.

## Transports {#transports}

The server exposes **streamable HTTP** at `/mcp`. Each request carries its own
bearer token, and the endpoint is stateless. Only loopback hosts and the host of
`SOLIDATE_PUBLIC_URL` are accepted, which protects local servers from DNS
rebinding.

For local use without a running server there is also a **stdio** binary,
`solidate-mcp`, which connects to the database directly. It reads
`DATABASE_URL` and `SOLIDATE_TOKEN` and checks the token at startup.

## Connecting Claude Code {#claude-code}

Create a write token for the project, then register the HTTP endpoint:

```sh
docker compose run --rm cli token create acme claude --scope write --project docs
claude mcp add --transport http solidate http://localhost:3000/mcp \
  --header "Authorization: Bearer sol_…"
```

The server sends instructions on connect that explain the two variants and the
translation workflow, so the agent does not need a custom prompt.

## Tools {#tools}

| Tool | Purpose |
|---|---|
| `list_projects` | Projects visible to the token, with parents |
| `list_docs` | Effective documents and the project root hash |
| `project_hash` | Merkle root; unchanged means nothing changed |
| `project_index` | The `llms.txt` index |
| `search` | Full-text search |
| `read_doc` | Read a variant or one section, with its content hash and outline |
| `write_doc` | Write a variant; `base_hash` required except on create |
| `backlinks` | Documents linking to a document |
| `doc_history` | Revisions of a variant |
| `get_sync_queue` | Sections awaiting translation |
| `get_sync_item` | Everything needed to translate one section |
| `resolve_sync` | Mark sections in sync without editing |
| `get_translation_guide` | How the variants differ in this project |
| `propose_translation` | Submit a translation for review |
| `list_proposals` | Open proposals, with diffs |

## Errors {#errors}

Failures come back as tool errors rather than protocol errors, so the model can
read them and react. A stale `base_hash`, for example, returns the current
content hash with an instruction to re-read and retry.
