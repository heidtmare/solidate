# ADR 0003: Tenant isolation with row-level security

- status: accepted

## Context {#context}

- multiple tenants, one database.
- app-level `WHERE tenant_id` filters: one omission = cross-tenant leak.

## Decision {#decision}

- role `solidate_app` (RLS applies); `SET ROLE` on connect.
- `TenantTx` sets `app.tenant_id` per transaction.
- policy `tenant_id = current_tenant()` on all tenant-scoped tables.
- composite FKs include `tenant_id`.
- pre-tenant lookups: `SECURITY DEFINER` functions.

## Consequences {#consequences}

- + missing filter -> empty result (fail closed); covers future queries.
- - migrations/tests need owner role.
- - new tenant-scoped tables MUST enable RLS + policy + grants.
