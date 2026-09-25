# Security and tenancy

Covers isolation, authentication, authorization, limits, audit.

## Tenant isolation {#isolation}

- all server connections: `SET ROLE solidate_app` (after connect).
- RLS policy `tenant_isolation` on every tenant-scoped table: `tenant_id = current_tenant()` (USING + WITH CHECK).
- `current_tenant()` = `current_setting('app.tenant_id')`; set per tx via `set_config('app.tenant_id', $1, true)` in `Db::tenant` -> `TenantTx`.
- unset -> zero rows (fail closed).
- composite FKs include `tenant_id`.
- pre-tenant lookups (`SECURITY DEFINER`): `tenant_by_slug`, `create_tenant`, `user_tenants`, `api_token_by_prefix`.
- migrations run as owner; `solidate_app` is `NOLOGIN`.

## People and sessions {#sessions}

- password hash: Argon2id (default params); min length 8.
- unknown email: verify dummy hash (timing-equalized).
- `users.disabled_at` set -> login rejected; sessions invalid.
- session lifetime: 14 days; stored as `token_hash`; cookie `Secure` unless `SOLIDATE_INSECURE_COOKIES=1`.

## API tokens {#tokens}

- format: `sol_<12 hex>_<64 hex>`; prefix `sol_<12 hex>` stored (unique); secret 256-bit.
- stored: BLAKE3(secret); compared constant-time (`subtle`).
- invalid if revoked or `expires_at <= now`.
- each use: `last_used_at` updated.

## Authorization {#authorization}

| actor | read | write | admin |
|---|---|---|---|
| user `reader` | yes | no | no |
| user `editor` | yes | yes | no |
| user `admin` | yes | yes | yes |
| token | scope >= `read` | scope >= `write` | scope `admin` |
| system (CLI) | yes | yes | yes |

- token `project` restriction: access only that project (+ read inherited docs from ancestors).
- admin-only: tokens, members, project settings, audit log.
- enforced in `solidate-app` `Ctx::require` for all front ends.

## Abuse controls {#limits}

- rate limit: in-process token bucket per token; 600/min, burst 60; 429 + `Retry-After`.
- max variant size: 1 MiB.
- Markdown rendering: raw HTML disabled.
- MCP HTTP: `Host` allow-list (loopback + `SOLIDATE_PUBLIC_URL` host).

## Audit log {#audit}

- one entry per mutation, same transaction.
- `solidate_app` grants: SELECT, INSERT only.
- actions listed in [[architecture/data-model#audit]].
