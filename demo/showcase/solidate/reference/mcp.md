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
| `read_doc` | Read a variant or one section (optionally with its subsections), with its content hash and outline |
| `write_doc` | Write a variant; `base_hash` required except on create |
| `write_section` | Replace, delete or insert one section without resending the rest |
| `backlinks` | Documents linking to a document |
| `doc_history` | Revisions of a variant |
| `get_sync_queue` | Sections awaiting translation |
| `get_sync_item` | Everything needed to translate one section |
| `resolve_sync` | Mark sections in sync without editing |
| `get_translation_guide` | How the variants differ in this project |
| `propose_translation` | Submit a translation for review |
| `list_proposals` | Open proposals, with diffs |
| `report_sources` | Report repository file hashes for drift tracking |
| `get_drift_queue` | Sections whose source files changed since verification |
| `affected_sections` | Sections that describe the given repository paths |
| `verify_sources` | Confirm sections still match their source files |

## Editing one section {#section-edits}

`write_section` lets an agent change part of a long document without resending
the whole of it. To replace a section, the agent reads it with
`read_doc(section=anchor)` and sends back only the new Markdown, heading
included, along with the section's `hash` as `section_hash`. The write then
fails only if that section changed in the meantime; edits to other sections do
not conflict. Passing the document's `content_hash` as `base_hash` instead
makes the write fail on any change. Setting `subsections` covers the section and
the deeper headings under it, for both the read and the write. Empty content
deletes the section.

To add a section, the agent passes `after` with an anchor, which inserts the new
section after that section and its subsections, or omits both `anchor` and
`after` to append. Inserted text must start with a heading, so it cannot run on
into the section before it.

The rest of the document is kept byte for byte. The server rejects an edit that
would change the anchor of any other section, as an unclosed code fence or a
duplicate heading would. The response lists the anchors that were written, and
keeping the heading text (or an explicit `{#anchor}`) keeps the section paired
with its translation. `resolves` works as it does for `write_doc`.

## Errors {#errors}

Failures come back as tool errors rather than protocol errors, so the model can
read them and react. A stale `base_hash`, for example, returns the current
content hash with an instruction to re-read and retry.
