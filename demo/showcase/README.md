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

## Recording

`recording/record.js` drives Chromium through the suggested recording and
writes a WebM with captions and a visible cursor: sign-in, project home, readme
in both variants, the sync queue, conflict details, accepting the staged agent
proposal, translating `diagrams` by hand, a human readme edit that an agent then
translates over REST, an agent proposal in the AI-to-human direction, history,
search and `llms.txt`. It changes state, so run `./seed.sh` first.

```sh
cd recording && npm i playwright
../seed.sh  # with SOLIDATE_TOKEN set to the docs-agent token
SOLIDATE_TOKEN=sol_... PW=<password> node record.js   # logs in as demo@solidate.dev
ffmpeg -i video/*.webm -c:v libx264 -crf 20 -pix_fmt yuv420p -movflags +faststart solidate-walkthrough.mp4
```

The root README's teaser GIF is cut from the MP4 (both variants, sync queue,
conflict, proposal review, `llms.txt`; 1.5x speed, 10 fps, 800 px wide):

```sh
ffmpeg -i solidate-walkthrough.mp4 -filter_complex "\
[0:v]trim=30:37,setpts=PTS-STARTPTS[a];[0:v]trim=40:48,setpts=PTS-STARTPTS[b];\
[0:v]trim=54:67,setpts=PTS-STARTPTS[c];[0:v]trim=158:164,setpts=PTS-STARTPTS[d];\
[a][b][c][d]concat=n=4:v=1,setpts=PTS/1.5,fps=10,scale=800:-1:flags=lanczos,split[x][y];\
[x]palettegen=max_colors=128:stats_mode=diff[p];[y][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
  -loop 0 solidate-teaser.gif
```

The trim points follow the current recording; re-check them after re-recording.
