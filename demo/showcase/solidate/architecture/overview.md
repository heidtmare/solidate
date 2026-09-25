# Architecture overview

Solidate is a Rust workspace of six crates backed by PostgreSQL. The crates form
strict layers: pure domain logic at the bottom, storage and use cases above it,
and three thin front ends on top that differ only in how they talk to callers.

## Layers {#layers}

```text
   solidate-web          solidate-mcp          solidate-cli
  (HTML, REST, /mcp)    (MCP tools, stdio)    (administration)
          \                    |                    /
           \                   |                   /
            +--------------- solidate-app -------+
            |   use cases, authorization, audit  |
            +------------------+------------------+
                               |
                         solidate-db
               PostgreSQL repositories, RLS, migrations
                               |
                        solidate-core
      paths, Markdown analysis, hashing, sync, includes, links
```

Each layer depends only on the ones below it. The web crate embeds the MCP crate
to serve `/mcp`, so a single server process provides the UI, the REST API and
MCP over HTTP.

## Crates {#crates}

**solidate-core** holds the domain rules and performs no I/O. It validates paths
and slugs, splits Markdown into sections with anchors, computes content and
semantic hashes, classifies sync state, expands includes and parses links.
Because it is pure, most of its behavior is covered by unit and property tests.

**solidate-db** is the storage layer. Every tenant-scoped query runs inside a
`TenantTx`, a transaction that has already set the current tenant, so row-level
security applies to everything it touches. It also owns the SQL migrations.

**solidate-app** implements the use cases: reading and writing documents,
inheritance, sync, proposals, search, tokens and audit. Front ends authenticate
the caller and build a `Ctx`; every authorization decision is made here, once,
for all front ends.

**solidate-web** is the HTTP server, built on Topcoat. It serves server-rendered
pages with htmx for the few interactive parts, the REST API under `/api/v1`, the
MCP endpoint and `/healthz`.

**solidate-mcp** maps MCP tools one to one onto app operations. It runs over
stdio as its own binary or inside the web server over streamable HTTP.

**solidate-cli** is the operator tool for migrations, tenants, users, projects,
tokens, import, export and audit.

## Request lifecycle {#lifecycle}

A request enters a front end, which authenticates it: a session cookie for the
browser, a bearer token for the API and MCP. The front end resolves the tenant
and builds a `Ctx` holding the tenant and the actor. It then calls a method on
`App`, which opens a tenant transaction, checks the actor's access against the
project, performs the work, writes an audit entry when something changed, and
commits. The front end turns the result into HTML, JSON or an MCP tool result.

The write path in detail is in [[architecture/write-path]]; isolation and
authentication are in [[architecture/security]].

## Binaries {#binaries}

The Docker image contains three binaries: `solidate-server`, `solidate` (the
CLI) and `solidate-mcp`. It runs as an unprivileged user and checks `/healthz`.
