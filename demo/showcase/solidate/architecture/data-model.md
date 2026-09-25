# Data model

All state lives in PostgreSQL. The schema separates a small set of global tables
from tenant-scoped tables, and it never rewrites stored content: documents are an
append-only history of revisions with a pointer to the current one.

## Global tables {#global}

`tenants`, `users`, `password_credentials`, `user_identities` and `sessions`
exist before any tenant is known, for example while someone signs in. Users are
global so that one person can belong to several tenants. Email addresses are unique regardless of case.
Passwords live in `password_credentials`, separate from users, so a user can exist
without one. `user_identities` links a user to external sign-in identities, each
an OpenID Connect issuer and subject. Session tokens and passwords are stored only
as hashes.

## Projects and documents {#documents}

A `project` belongs to a tenant, has a slug unique within it, an optional parent
project and a JSON `settings` object. A `document` belongs to a project and is
identified by its path. Deleting a document marks it deleted rather than removing
its rows, and path uniqueness applies only to live documents, so a path can be
reused.

## Revisions and heads {#revisions}

Every save of a variant creates a `revision` that records the content hash, the
semantic hash, its parent revision, the author (a user or a token) and an
optional change note. The Markdown itself lives in `blobs`, keyed by content
hash, so identical content is stored once.

A `head` points at the current revision of each variant and caches what reads
need: the title and a full-text search vector. `sections` stores the analyzed
outline of every revision, including each section's anchor, level, parent and
semantic hash; section bodies are sliced from the blob on demand. `links` stores
the outgoing links of each head for backlinks.

## Sync and proposals {#sync}

`sync_bases` holds, per document and anchor, the semantic hash each side had at
the last reconciliation. `proposals` holds at most one open translation proposal
per document variant, together with the hashes of both heads at submission time,
which is how Solidate knows a proposal is outdated.

## Tokens and audit {#audit}

`api_tokens` stores a public lookup prefix, a hash of the secret, scopes, an
optional project restriction, an expiry and revocation time. `audit_log` is
append-only for the application: it can insert and read entries but never update
or delete them. Audit ids are time-ordered UUIDv7 values, so the primary key
doubles as the chronological index, and entries carry no foreign keys so they
outlive the users and tokens they mention.

## Tenant keys everywhere {#tenant-keys}

Every tenant-scoped table carries `tenant_id`, defaulting to the current tenant,
and foreign keys include it. A row therefore cannot reference a row in another
tenant, even through a bug. How the current tenant is set is described in
[[architecture/security#isolation]].
