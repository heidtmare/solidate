# Administration

Binary: `solidate`. Connects via `DATABASE_URL`; system privileges (no authz). Compose: `docker compose run --rm cli <subcommand>`.

## Migrations {#migrations}

- `solidate migrate`: apply pending migrations; run as DB owner.
- Compose runs it before `app` starts. Server connections use role `solidate_app`.

## Tenants, users and members {#people}

```sh
solidate tenant create acme "Acme Corp"
solidate user create ada@acme.dev "Ada Lovelace"
solidate user passwd ada@acme.dev
solidate user member acme ada@acme.dev editor
```

- password source: `SOLIDATE_PASSWORD`, else stdin.
- users: global; one role per tenant.
- roles: `reader` (read) < `editor` (+write) < `admin` (+tokens, members).

## Projects {#projects}

```sh
solidate project create acme docs "Documentation" --parent handbook
solidate project list acme
solidate project set-parent acme docs handbook
```

- `set-parent` without parent -> root.

## API tokens {#tokens}

```sh
solidate token create acme docs-agent --scope write --project docs --expires-days 90
solidate token list acme
solidate token revoke acme <token-id>
```

- `--scope` repeatable, required; `read|write|admin`.
- secret printed once (stdout); stored as BLAKE3 hash. See [[architecture/security#tokens]].

## Import and export {#import-export}

```sh
solidate import acme docs ./docs --dry-run
solidate import acme docs ./docs --synced
solidate export acme docs ./backup
```

- mapping: `x.md` -> `x` human; `x.ai.md` -> `x` ai.
- skips hidden files, invalid paths; unchanged content not rewritten (idempotent).
- order: human variants first.
- `--synced`: reconcile sections present in both variants.
- export: own documents only (no inherited), same layout.
- this project: loaded with `import --synced`.

## Sync queue and audit {#observe}

```sh
solidate sync-queue acme docs
solidate audit acme --limit 20
```

- `sync-queue` output: `path#anchor\tstate\tstale=<side|both>`.
- `audit` output (TSV, newest first): `id at actor action project target detail`; paging `--before <id>`; default limit 50.
