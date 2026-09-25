# Data model

PostgreSQL. Global vs tenant-scoped tables. Content never rewritten: append-only revisions + head pointer.

## Global tables {#global}

| table | key columns | notes |
|---|---|---|
| `tenants` | `id`, `slug` UNIQUE, `name` | RLS: `id = current_tenant()` for `solidate_app` |
| `users` | `id`, `email`, `name`, `disabled_at` | unique `lower(email)`; global (multi-tenant membership) |
| `password_credentials` | `user_id` PK, `hash` (argon2id), `updated_at` | optional per user; no row = no password sign-in |
| `sessions` | `token_hash` PK, `user_id`, `expires_at` | token stored hashed |

## Projects and documents {#documents}

| table | key columns | notes |
|---|---|---|
| `memberships` | `(tenant_id, user_id)`, `role` | `reader\|editor\|admin` |
| `projects` | `id`, `slug` (unique per tenant), `parent_id`, `settings` jsonb | parent FK `ON DELETE RESTRICT`; no self-parent |
| `documents` | `id`, `project_id`, `path`, `title`, `sync_enabled`, `deleted_at` | soft delete; unique `(tenant_id, project_id, path) WHERE deleted_at IS NULL` |

## Revisions and heads {#revisions}

| table | key columns | notes |
|---|---|---|
| `blobs` | `(tenant_id, hash)`, `content`, `byte_len` | content-addressed; dedup |
| `revisions` | `id`, `document_id`, `variant`, `content_hash`, `semantic_hash`, `parent_id`, `author_user_id` \| `author_token_id`, `message` | index `(document_id, variant, created_at DESC)` |
| `heads` | `(tenant_id, document_id, variant)`, `revision_id`, `content_hash`, `title`, `search` tsvector | GIN index on `search` |
| `sections` | `(revision_id, ordinal)`, `anchor`, `title`, `level`, `parent_anchor`, `hash` | per revision; bodies sliced from blob |
| `links` | `(document_id, variant, ordinal)`, `target_project`, `target_path`, `target_anchor` | per head; backlinks; `target_project NULL` = own project |

## Sync and proposals {#sync}

| table | key columns | notes |
|---|---|---|
| `sync_bases` | `(document_id, anchor)`, `human_hash`, `ai_hash` | semantic hashes at last reconciliation |
| `proposals` | `id`, `document_id`, `variant`, `base_hash`, `source_hash`, `content`, `resolves[]`, `message` | UNIQUE `(tenant_id, document_id, variant)`; outdated if either head hash differs |

## Tokens and audit {#audit}

| table | key columns | notes |
|---|---|---|
| `api_tokens` | `id`, `prefix` UNIQUE, `secret_hash`, `scopes[]`, `project_id`, `expires_at`, `last_used_at`, `revoked_at` | scopes subset of `read,write,admin` |
| `audit_log` | `id` (UUIDv7), `at`, `actor_kind` (`user\|token\|system`), `actor_user_id`, `actor_token_id`, `action`, `project_id`, `target`, `detail` jsonb | `solidate_app`: SELECT, INSERT only; no FKs on actors |

- audit actions: `tenant.create`, `member.set`, `project.create`, `project.set_parent`, `project.settings`, `token.create`, `token.revoke`, `doc.write`, `doc.delete`, `doc.sync_setting`, `sync.resolve`, `proposal.submit`, `proposal.accept`, `proposal.reject`.

## Tenant keys everywhere {#tenant-keys}

- every tenant-scoped table: `tenant_id DEFAULT current_tenant()`.
- composite FKs include `tenant_id` -> cross-tenant references impossible.
- tenant selection: [[architecture/security#isolation]].
