# Showcase

Solidate's own documentation, stored in Solidate, for screenshots and recordings.
`./seed.sh` loads it into tenant `heidtmare`: project `handbook` (glossary, style
rules, translation guide) and its child `solidate` (guides, reference,
architecture, ADRs). Each document has a human (`x.md`) and AI (`x.ai.md`)
variant. The script then uses an agent token to leave the project in a mixed
sync state.

## Staged state

| Document | Section | State |
|---|---|---|
| `architecture/hashing` | `semantic` | conflict |
| `guide/quickstart` | `start` | human ahead, AI proposal pending |
| `guide/writing-documents` | `diagrams` | human ahead, AI section missing |
| `reference/configuration` | `limits` | AI ahead |
| `guide/agents-and-translation` | `guardrails` | in sync after a bullet-style-only edit |
| `readme` | `features` | in sync; both variants revised by the agent |

## Shot list

Base URL `http://localhost:3000/t/heidtmare/p/solidate`.

| Shot | Path | Shows |
|---|---|---|
| Project home | `/` | document tree, own and inherited |
| Readme | `/d/readme` | rendering, outline, include from `handbook:glossary`, Includes panel |
| Readme, AI tab | `/d/readme?v=ai` | the same facts as dense reference |
| Inherited page | `/d/glossary` | "Inherited from handbook" notice, Override button |
| Backlinks | `/d/architecture/hashing` | "Linked from" panel, conflict dot in outline |
| Sync queue | `/sync` | all four states plus the pending proposal |
| Conflict item | `/sync/architecture/hashing` | both sides, base text, diffs |
| Proposal review | `/sync/guide/quickstart` | proposal diff, Accept / Reject |
| Editor | `/edit/guide/writing-documents?v=ai` | live preview while writing the missing section |
| History | `/history/readme` | revisions by `docs-agent` with change notes |
| Search | `/t/heidtmare/search?q=merkle` | highlighted snippets across variants |
| llms.txt | `/llms.txt` | index for language models |

Suggested recording: open the sync queue, accept the quickstart proposal, then
write the missing `diagrams` section from the sync item and watch the queue
shrink. Re-run `./seed.sh` to reset.
