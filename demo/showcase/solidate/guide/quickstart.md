# Quickstart

This guide takes you from a clone of the repository to a running server with a
tenant, a user, a project and a first document. It assumes Docker with Compose.

## Start the stack {#start}

From the repository root:

```sh
docker compose up -d
```

Compose starts PostgreSQL 17, runs the one-shot `cli` service to apply
migrations, and then starts the server on <http://localhost:3000>. PostgreSQL is
published on host port 5433 so it does not collide with a local installation.
The server reports health at `/healthz`.

## Create a tenant and a user {#tenant}

Administration happens through the `solidate` CLI, which Compose exposes as the
`cli` service. It connects with system privileges, so it bypasses per-user
permissions.

```sh
docker compose run --rm cli tenant create acme "Acme Corp"
docker compose run --rm -e SOLIDATE_PASSWORD=change-me-now cli user create ada@acme.dev "Ada Lovelace"
docker compose run --rm cli user member acme ada@acme.dev admin
```

Passwords must be at least eight characters. When `SOLIDATE_PASSWORD` is not set,
the CLI reads the password from standard input.

## Create a project {#project}

```sh
docker compose run --rm cli project create acme docs "Documentation"
```

Add `--parent <slug>` to make the new project inherit from an existing one.

## Write your first document {#first-document}

Sign in at <http://localhost:3000/login>, open the project, and choose **New
document**. Give it a path such as `design/auth`, write the human variant, and
save. Then open the **AI** tab and write its counterpart. Sections present in
both variants start out in sync.

To load an existing folder of Markdown instead, see
[[guide/administration#import-export]].

## Connect an agent {#agent}

Create an API token and point an MCP client at `/mcp`. The steps are in
[[reference/mcp]].
