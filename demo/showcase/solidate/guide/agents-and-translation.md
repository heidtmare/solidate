# Agents and translation

Agents are first-class authors in Solidate. They read and write documents through
the REST API or MCP with their own API token, and every revision they make is
attributed to that token. This page describes the workflow Solidate expects from
an agent, and how people stay in control of it.

## Two kinds of agent work {#kinds}

An agent that changes what a document says is an author like any other. It
should update both variants in one go: write the variant it edited, then write
the translation and list the translated anchors as `resolves`, so the pair never
shows up as stale.

An agent can also act as a translator for changes other people made. That work
goes through proposals, because a person should confirm that a translation
carries the meaning over faithfully.

## The translation guide {#guide}

Before translating, an agent reads the project's translation guide. It is an
ordinary document at `_meta/translation`, resolved through inheritance, so one
guide in a parent project applies to all of its children. This project inherits
its guide from `handbook`. When no project in the chain defines one, Solidate
falls back to a built-in default.

## The translation loop {#loop}

1. Read the translation guide once per project.
2. Take stale sections from the sync queue, skipping any already covered by a
   current proposal.
3. For each, fetch the sync item: both variants' current text, the text at the
   last sync, the diff since, and each variant's content hash.
4. Write the full stale variant with the changes carried over, and submit it as a
   proposal with the content hash it replaces and the anchors it resolves.

A new proposal for the same variant replaces the previous one, so an agent can
safely retry.

## Reviewing proposals {#review}

Open proposals appear on the project's **Sync** page with a diff against the
current variant. Accepting one writes it as a new revision, authored by the
reviewer and noting the proposer in the audit log, and marks its sections in
sync. Rejecting it simply deletes it.

A proposal becomes *outdated* as soon as either variant changes after it was
submitted, because the translation was made against text that no longer exists.
Outdated proposals cannot be accepted; the agent resubmits against the new text.

## Guardrails {#guardrails}

- Tokens carry scopes (`read`, `write`, `admin`) and can be restricted to one
  project and given an expiry.
- Every write uses optimistic concurrency: an agent must send the hash of the
  content it read, so it can never overwrite a change it has not seen.
- Requests are rate limited per token.
- Every mutation is recorded in the tenant's audit log with the token that made it.

Details are in [[architecture/security]] and [[reference/mcp]].
