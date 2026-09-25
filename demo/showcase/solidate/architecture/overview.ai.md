# Architecture overview

Rust workspace, 6 crates, PostgreSQL 17. Strict layering; front ends thin.

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

- deps point downward only. `solidate-web` embeds `solidate-mcp` for `/mcp`.
- one server process: UI + REST + MCP-over-HTTP.

## Crates {#crates}

| crate | role | key modules / types |
|---|---|---|
| `solidate-core` | pure domain, no I/O | `path` (`DocPath`, `Slug`), `markdown` (`analyze`, `render_html`), `hash` (`Hash`, `Merkle`), `sync` (`classify`, `plan`, `reconcile`), `include`, `links`, `inherit`, `diff` |
| `solidate-db` | storage | `Db`, `TenantTx` (RLS-scoped tx), `migrations/` |
| `solidate-app` | use cases + authz | `App`, `Ctx`, `Actor`, `Access`; docs, sync, proposals, search, auth, audit, ratelimit |
| `solidate-web` | HTTP (Topcoat, htmx) | `pages/`, `api/` (`/api/v1`), `mcp.rs` (`/mcp`), `/healthz` |
| `solidate-mcp` | MCP tools (rmcp) | `SolidateMcp`, `http_service` |
| `solidate-cli` | operator CLI | `import`/`export` file mapping |

- core: unit + proptest coverage.
- all authorization in `solidate-app` (`Ctx::require`).

## Request lifecycle {#lifecycle}

1. front end authenticates: session cookie (web) | bearer token (API, MCP).
2. build `Ctx { tenant, principal, grant }`; principal = author identity (user | token | system), grant = level (role | scopes | unrestricted) + optional project restriction.
3. call `App` method -> open `TenantTx` -> `ctx.require(access, project)` -> work -> audit entry (if mutating) -> commit.
4. render HTML | JSON | MCP result.

- write path: [[architecture/write-path]]; isolation/auth: [[architecture/security]].

## Binaries {#binaries}

- image: `solidate-server`, `solidate`, `solidate-mcp`.
- runs as uid 10001; `HEALTHCHECK` curls `/healthz`.
