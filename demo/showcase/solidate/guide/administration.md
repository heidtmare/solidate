# Administration

Solidate is administered with the `solidate` command-line tool. It connects
directly to PostgreSQL with `DATABASE_URL` and acts with system privileges, so it
is meant for operators, not end users. Under Compose, run it as
`docker compose run --rm cli <subcommand>`.

## Migrations {#migrations}

`solidate migrate` applies pending database migrations. Compose runs it
automatically before the server starts. Migrations run as the database owner;
the server itself runs as the restricted `solidate_app` role.

## Tenants, users and members {#people}

```sh
solidate tenant create acme "Acme Corp"
solidate user create ada@acme.dev "Ada Lovelace"
solidate user passwd ada@acme.dev
solidate user member acme ada@acme.dev editor
```

Users are global and can belong to several tenants, with one role in each:
*reader* can read, *editor* can also write, *admin* can also manage tokens and
members.

## Projects {#projects}

```sh
solidate project create acme docs "Documentation" --parent handbook
solidate project list acme
solidate project set-parent acme docs handbook
```

Omitting the parent in `set-parent` makes the project a root again.

## API tokens {#tokens}

```sh
solidate token create acme docs-agent --scope write --project docs --expires-days 90
solidate token list acme
solidate token revoke acme <token-id>
```

The secret is printed once and never stored; only its hash is kept. See
[[architecture/security#tokens]].

## Import and export {#import-export}

Import loads a directory of Markdown files into a project. A file `x.md` becomes
the human variant of document `x`, and `x.ai.md` its AI variant. Hidden files are
skipped, and files that are already current are left untouched, so imports can be
repeated.

```sh
solidate import acme docs ./docs --dry-run
solidate import acme docs ./docs --synced
solidate export acme docs ./backup
```

`--synced` marks sections present in both variants as in sync, which is what you
want when importing documentation that is already consistent. Export writes the
project's own documents, not inherited ones, in the same layout. This project
was loaded with `import --synced`.

## Sync queue and audit {#observe}

```sh
solidate sync-queue acme docs
solidate audit acme --limit 20
```

The audit log prints newest first, tab-separated: id, time, actor, action,
project, target and detail. Pass `--before <id>` to page.
