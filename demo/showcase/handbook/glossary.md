# Glossary

Shared vocabulary for every project in this tenant. Project documents include the
sections they need with `{{include handbook:glossary#core-terms}}` rather than
redefining terms, so a definition changes in one place.

## Core terms {#core-terms}

**Tenant.** An isolated organization. Every row of tenant data carries the tenant's
id, and the database itself refuses to return another tenant's rows.

**Project.** A named collection of documents inside a tenant. Projects can have a
parent project, and a child sees every document of its ancestors unless it
defines its own document at the same path.

**Document.** A Markdown page identified by a path such as `architecture/hashing`.
Each document is written twice, as a human variant and an AI variant.

**Variant.** One of the two renderings of a document. The *human* variant is
narrative prose; the *AI* variant is dense, structured reference. They state the
same facts.

**Section.** The part of a document that starts at a heading and runs to the next
heading. Sections are the unit Solidate tracks for sync.

**Anchor.** A section's stable identifier, taken from an explicit `{#id}` on the
heading or derived from the heading text. Anchors pair a human section with its AI
counterpart.

## Sync terms {#sync-terms}

**Sync base.** The semantic hashes both sides of a section pair had when they were
last agreed to be in sync.

**Stale side.** The variant that has fallen behind and needs a translation.

**Proposal.** A complete replacement of one variant, usually submitted by an
agent, that a person reviews and accepts or rejects.

**Translation guide.** The document at `_meta/translation` that tells agents how
the two variants of a project differ.
