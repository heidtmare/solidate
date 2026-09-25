# Quickstart

Clone -> running server with tenant, user, project, first document. Requires Docker Compose.

## Start the stack {#start}

```sh
docker compose up -d
```

- services: `postgres` (17; host port 5433), `cli` (one-shot `migrate`), `app` (http://localhost:3000).
- health: `GET /healthz`.

## Create a tenant and a user {#tenant}

CLI runs with system privileges (no permission checks).

```sh
docker compose run --rm cli tenant create acme "Acme Corp"
docker compose run --rm -e SOLIDATE_PASSWORD=change-me-now cli user create ada@acme.dev "Ada Lovelace"
docker compose run --rm cli user member acme ada@acme.dev admin
```

- password: min 8 chars; from `SOLIDATE_PASSWORD`, else stdin.

## Create a project {#project}

```sh
docker compose run --rm cli project create acme docs "Documentation"
```

- `--parent <slug>`: inherit from existing project.

## Write your first document {#first-document}

- UI: `/login` -> project -> New document -> path (e.g. `design/auth`) -> write human variant -> save -> AI tab -> write AI variant.
- sections present in both variants on first AI write: marked in sync.
- bulk import: [[guide/administration#import-export]].

## Connect an agent {#agent}

- create API token; MCP endpoint `/mcp`. See [[reference/mcp]].
