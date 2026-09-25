# Security and tenancy

Solidate is multi-tenant from the ground up. This page describes how tenants are
kept apart, how callers authenticate, what they are allowed to do, and what is
recorded about it.

## Tenant isolation {#isolation}

Isolation is enforced by PostgreSQL, not by application code remembering to add
a `WHERE` clause. Every connection the server opens switches to the restricted
role `solidate_app`, and every tenant-scoped table has a row-level security
policy that shows that role only rows whose `tenant_id` matches the current
tenant setting.

The application reaches tenant data only through a *tenant transaction*, which
sets that value for the lifetime of the transaction before running any query. If
the value is not set, no rows match at all, so a forgotten tenant fails closed.
Foreign keys include the tenant id, so even a direct insert cannot point one
tenant's row at another's.

A few lookups must happen before the tenant is known: finding a tenant by slug,
listing a user's tenants at sign-in, and finding a token by its prefix. These go
through narrow `SECURITY DEFINER` functions that return only what the lookup
needs.

## People and sessions {#sessions}

Users sign in with email and password, or through an OpenID Connect provider
when one is configured. Passwords are hashed with Argon2id, and a sign-in attempt
for an unknown email still verifies a dummy hash so that response times do not
reveal which accounts exist. Disabled users cannot sign in, and their sessions
stop working. Sessions last fourteen days and are stored only as hashes.

## Single sign-on {#sso}

OpenID Connect sign-in uses the authorization code flow with PKCE. The state,
nonce and PKCE verifier of an attempt are kept in a ten-minute `HttpOnly` cookie
scoped to `/login/oidc`; the callback is accepted only when its state matches
that cookie. The ID token's signature, issuer, audience, expiry and nonce are
verified, and its access token hash when present. When a token is signed with an
unknown key, the provider's keys are fetched again once, which handles key
rotation.

The provider's issuer and subject identify the person. The first sign-in with a
new identity requires an email address the provider marks as verified: the
identity is linked to the user with that address, or a user without a password
is created. Later sign-ins use the link and ignore the email. New users belong to
no tenant until an administrator adds them. Linking by email trusts the provider
to verify addresses, so configure only a provider that does.

## API tokens {#tokens}

A token looks like `sol_<12 hex>_<64 hex>`. The first part is a public prefix
used to find the token; the rest is a 256-bit secret. Solidate stores a BLAKE3
hash of the secret and compares it in constant time. A fast hash is enough here
because the secret is random, unlike a password. Tokens can be revoked and can
expire, and each use updates `last_used_at`.

## Authorization {#authorization}

Every operation declares the access it needs, read, write or admin, and the app
layer checks it against the caller before touching data.

- **Users** can read everything in their tenant. Editors and admins can write;
  only admins can manage tokens, members and project settings.
- **Tokens** carry scopes, where `admin` implies `write` and `write` implies
  `read`. A token restricted to one project can access only that project,
  and can still read documents it inherits from ancestors.
- **The CLI** acts as the system and is unrestricted; it is meant for operators
  with database access.

## Abuse controls {#limits}

API and MCP requests are limited per token with a token bucket, by default 600
requests per minute with bursts of 60. Documents are capped at 1 MiB per variant.
Rendered Markdown never includes raw HTML, so content written by anyone,
including an agent, is safe to display. The MCP endpoint accepts only expected
`Host` headers to block DNS rebinding.

## Audit log {#audit}

Every mutation, including document writes, sync resolutions, proposal decisions,
token changes and membership changes, writes an audit entry in the same
transaction as the change, so the log and the data cannot disagree. The
application role can add entries but not change or delete them.
