# Solidate

Solidate is a wiki-style knowledge base for project design documentation that
people and AI agents both read and write. Every document is one ground truth
written twice: a human variant in narrative prose, and an AI variant that states
the same facts as dense, structured reference. Solidate tracks the two variants
section by section and shows exactly which side has fallen behind.

This project documents Solidate itself, and it is stored in Solidate. The page
you are reading has an **AI** tab with its counterpart.

## Why two variants {#why}

Documentation written for people explains context and trade-offs. Agents work
better from terse lists of exact names, paths and limits. Keeping one document
that serves both audiences produces something neither reads well, and keeping two
separate documents lets them drift apart silently.

Solidate keeps both, pairs their sections by anchor, and hashes each section. When
someone edits one side, the other side is flagged as stale until it is translated
or explicitly marked in sync. The reasoning is recorded in
[[decisions/0002-dual-variants]].

## What you get {#features}

- **Server-rendered UI.** Fast HTML pages with no client-side framework; htmx for
  the few interactive parts.
- **Section-level sync.** A queue of stale sections, side-by-side diffs, and one
  click to mark a formatting-only change as in sync.
- **Agent translation with review.** Agents submit translations as proposals;
  a person accepts or rejects them.
- **Project inheritance.** Child projects inherit documents, including a shared
  translation guide and style rules, and can override any of them.
- **Transclusion and links.** `{{include}}` directives, wiki links, backlinks.
- **Content hashing.** BLAKE3 content hashes as ETags, semantic hashes for sync,
  and a Merkle root per project.
- **APIs for tools.** A REST API, an MCP server, and an `llms.txt` index.
- **Multi-tenant by construction.** PostgreSQL row-level security isolates tenants.

## Vocabulary {#vocabulary}

The terms below come from the tenant-wide glossary in the parent project
`handbook`, included here rather than copied:

{{include handbook:glossary#core-terms}}

## Where to go next {#next}

- New here: [[guide/quickstart]], then [[guide/writing-documents]].
- Understanding sync: [[guide/variants-and-sync]] and
  [[guide/agents-and-translation]].
- Running it: [[guide/administration]] and [[reference/configuration]].
- Integrating: [[reference/rest-api]] and [[reference/mcp]].
- How it is built: [[architecture/overview]].
