#!/usr/bin/env bash
# Seeds the showcase: Solidate's own documentation, stored in Solidate.
#
#   tenant    heidtmare
#   projects  handbook (root: glossary, style rules, translation guide)
#             solidate (child of handbook: guides, reference, architecture, ADRs)
#
# After importing everything in sync, an agent token makes a few edits through the
# REST API so the UI shows every sync state: human ahead, AI ahead, conflict, a
# section missing its AI counterpart, a pending translation proposal, and a
# formatting-only edit that stays in sync.
#
# Requires the Compose stack (`docker compose up -d`), python3 and an existing
# tenant `heidtmare`. Re-running is safe: imports and edits are idempotent, the
# proposal is replaced. Set SOLIDATE_TOKEN to reuse an agent token instead of
# creating a new one.
set -euo pipefail

cd "$(dirname "$0")"
HERE="$PWD"
ROOT="$(cd ../.. && pwd)"
TENANT="${TENANT:-heidtmare}"
API="${API:-http://localhost:3000/api/v1}"

cli() { (cd "$ROOT" && docker compose run --rm -T -v "$HERE:/showcase:ro" cli "$@"); }

if ! cli project list "$TENANT" | awk '$1 == "handbook" { found = 1 } END { exit !found }'; then
  cli project create "$TENANT" handbook "Engineering Handbook"
fi
cli project set-parent "$TENANT" solidate handbook

cli import "$TENANT" handbook /showcase/handbook --synced
cli import "$TENANT" solidate /showcase/solidate --synced

if [[ -z "${SOLIDATE_TOKEN:-}" ]]; then
  SOLIDATE_TOKEN="$(cli token create "$TENANT" docs-agent --scope write --project solidate 2>/dev/null | tail -n1)"
  echo "agent token: $SOLIDATE_TOKEN"
fi
export SOLIDATE_TOKEN API HERE

python3 - <<'PY'
import json, os, urllib.error, urllib.parse, urllib.request

API, TOKEN, HERE = os.environ["API"], os.environ["SOLIDATE_TOKEN"], os.environ["HERE"]
P = "solidate"


def call(method, path, body=None, headers=None, ctype="application/json"):
    h = {"Authorization": f"Bearer {TOKEN}", **(headers or {})}
    data = None
    if body is not None:
        data = body.encode() if isinstance(body, str) else json.dumps(body).encode()
        h["Content-Type"] = ctype
    req = urllib.request.Request(API + path, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        raise SystemExit(f"{method} {path}: {e.code} {e.read().decode()}")


def source(path, variant):
    suffix = ".ai.md" if variant == "ai" else ".md"
    return open(f"{HERE}/{P}/{path}{suffix}").read()


def head(path, variant):
    return call("GET", f"/projects/{P}/docs/{path}?variant={variant}&format=json")["content_hash"]


def write(path, variant, content, message, resolves=()):
    q = urllib.parse.urlencode({"variant": variant, "message": message, "resolves": ",".join(resolves)})
    r = call("PUT", f"/projects/{P}/docs/{path}?{q}", content, {"If-Match": "*"}, "text/markdown")
    print(f"  {path} ({variant}): changed={r['changed']} pending={[s['anchor'] + ':' + s['state'] for s in r['sync']]}")


def edit(path, variant, old, new, message, resolves=()):
    text = source(path, variant)
    assert old in text, (path, variant, old)
    write(path, variant, text.replace(old, new), message, resolves)
    return text.replace(old, new)


print("history: readme edited in both variants, kept in sync")
edit("readme", "human",
     "- **APIs for tools.** A REST API, an MCP server, and an `llms.txt` index.",
     "- **APIs for tools.** A REST API, an MCP server over HTTP and stdio, and an\n  `llms.txt` index that points language models at the AI variants.",
     "Mention MCP transports and what llms.txt links to")
edit("readme", "ai",
     "| APIs | REST `/api/v1`, MCP (`/mcp`, stdio), `llms.txt` |",
     "| APIs | REST `/api/v1`, MCP (`/mcp` streamable HTTP, `solidate-mcp` stdio), `llms.txt` (links AI variants) |",
     "Translate: MCP transports, llms.txt targets", resolves=["features"])

print("formatting-only edit: stays in sync (semantic hash unchanged)")
text = source("guide/agents-and-translation", "human")
head_, _, tail = text.partition("## Guardrails {#guardrails}")
guard, sep, rest = tail.partition("Details are in")
write("guide/agents-and-translation", "human",
      head_ + "## Guardrails {#guardrails}" + guard.replace("\n- ", "\n* ") + sep + rest,
      "Use asterisk bullets")

print("human ahead + pending proposal: quickstart#start")
edit("guide/quickstart", "human",
     "The server reports health at `/healthz`.",
     "The server reports health at `/healthz`. Follow its logs with\n`docker compose logs -f app`; set `SOLIDATE_LOG_FORMAT=json` for structured output.",
     "Add log-following tip")
ai = source("guide/quickstart", "ai").replace(
    "- health: `GET /healthz`.",
    "- health: `GET /healthz`.\n- logs: `docker compose logs -f app`; structured: `SOLIDATE_LOG_FORMAT=json`.")
call("POST", f"/projects/{P}/propose/guide/quickstart", {
    "variant": "ai",
    "content": ai,
    "base_hash": head("guide/quickstart", "ai"),
    "message": "Carry over the log-following tip from the human variant.",
    "resolves": ["start"],
})
print("  proposal submitted for guide/quickstart (ai)")

print("AI ahead: configuration#limits")
edit("reference/configuration", "ai",
     "- not runtime-configurable.",
     "- request body: bounded by `max_doc_bytes` for document writes and proposals.\n- not runtime-configurable.",
     "Note proposal size limit")

print("conflict: hashing#semantic edited on both sides")
edit("architecture/hashing", "human",
     "Sync compares semantic hashes.",
     "Sync compares semantic hashes, section by section.",
     "Clarify sync granularity")
edit("architecture/hashing", "ai",
     "- used by sync (`sections.hash`, `sync_bases`).",
     "- used by sync (`sections.hash`, `sync_bases`); compared per anchor.",
     "Clarify comparison unit")

print("human-only section: writing-documents#diagrams")
edit("guide/writing-documents", "human",
     "[^1]: Footnotes render at the end of the page.\n",
     "[^1]: Footnotes render at the end of the page.\n\n## Diagrams {#diagrams}\n\n"
     "Fenced code blocks tagged `text` keep their layout, which is enough for box\n"
     "diagrams like the one in [[architecture/overview#layers]]. Mermaid blocks are\n"
     "shown as code.\n",
     "Document diagram options")

q = call("GET", f"/projects/{P}/sync")
print(f"sync queue: {len(q)} sections")
for e in q:
    print(f"  {e['path']}#{e['anchor']}: {e['state']}")
PY
