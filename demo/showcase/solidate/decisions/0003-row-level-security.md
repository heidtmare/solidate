# ADR 0003: Tenant isolation with row-level security

**Status:** Accepted

## Context {#context}

Solidate hosts several tenants on one database. Filtering by tenant in every
query works until one query forgets, and the consequence of that bug is a data
leak between organizations.

## Decision {#decision}

Enforce isolation in PostgreSQL. The server connects as a restricted role subject
to row-level security; each tenant transaction sets the current tenant; policies
limit every tenant-scoped table to that tenant; foreign keys include the tenant
id. Lookups needed before a tenant is known go through small `SECURITY DEFINER`
functions.

## Consequences {#consequences}

A missing tenant filter returns nothing instead of another tenant's data, and the
guarantee holds for every query, including ones written later. Migrations and
tests need the owner role, and every tenant-scoped table must be added to the
policy list when it is created.
