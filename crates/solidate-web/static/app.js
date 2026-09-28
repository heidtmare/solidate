// Renders `pre.mermaid` blocks. mermaid.js loads on first use, from the URL in this
// script's `data-mermaid` attribute, and runs again after every htmx swap. Each block
// renders independently: one that fails is replaced by its error message and numbered
// source, with the last version that rendered at the same position, if any.
(() => {
  const src = document.currentScript.dataset.mermaid;
  // Source -> SVG. Preview swaps re-send every diagram; unchanged ones skip rendering.
  const svgs = new Map();
  // Root element -> SVG per diagram position from its previous render.
  const lastGood = new WeakMap();
  let loading;
  let seq = 0;

  function load() {
    loading ??= new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = src;
      s.onload = () => {
        const dark = matchMedia("(prefers-color-scheme: dark)").matches;
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          suppressErrorRendering: true,
          theme: dark ? "dark" : "default",
        });
        resolve(mermaid);
      };
      s.onerror = reject;
      document.head.append(s);
    });
    return loading;
  }

  async function svgFor(m, source) {
    let svg = svgs.get(source);
    if (svg === undefined) {
      await m.parse(source);
      ({ svg } = await m.render(`mermaid-${++seq}`, source));
      if (svgs.size >= 100) svgs.delete(svgs.keys().next().value);
      svgs.set(source, svg);
    }
    return svg;
  }

  // Maps a line of the text mermaid parses back to a line of `source`. Before parsing,
  // mermaid removes front matter, directives, `%%` comment lines, and leading blank lines.
  function sourceLine(source, parsed) {
    const lines = source.split("\n");
    const front = /^-{3}\s*\n[\s\S]*?\n-{3}\s*\n+/.exec(source);
    let i = front ? front[0].split("\n").length - 1 : 0;
    let n = 0;
    for (; i < lines.length; i++) {
      const l = lines[i];
      if (/^\s*%%(?!\{)./.test(l)) continue;
      if (n === 0 && /^\s*(%%\{.*\}%%)?\s*$/.test(l)) continue;
      if (++n === parsed) return i + 1;
    }
    return null;
  }

  function errorBlock(node, source, error, previous) {
    const message = String(error?.message ?? error).trim();
    const reported = /\bline (\d+)/i.exec(message);
    const line = reported ? sourceLine(source, Number(reported[1])) : null;
    const start = Number(node.dataset.line) || null;

    const box = document.createElement("div");
    box.className = "diagram-error";
    if (start) box.dataset.line = line ? start + line - 1 : start;

    const title = document.createElement("p");
    const strong = document.createElement("strong");
    strong.textContent = "Diagram error";
    title.append(strong);
    if (box.dataset.line) title.append(` on line ${box.dataset.line}`);

    const detail = document.createElement("pre");
    detail.className = "message";
    detail.textContent = message;

    const code = document.createElement("pre");
    code.className = "numbered";
    for (const [i, text] of source.replace(/\n$/, "").split("\n").entries()) {
      const span = document.createElement("span");
      span.textContent = text;
      if (i + 1 === line) span.className = "err";
      code.append(span);
    }
    box.append(title, detail, code);

    if (previous) {
      const note = document.createElement("p");
      note.className = "muted small";
      note.textContent = "Last version that rendered:";
      const figure = document.createElement("div");
      figure.className = "previous";
      figure.innerHTML = previous;
      box.append(note, figure);
    }
    node.replaceWith(box);
  }

  // Lists diagram errors next to the editor's Save button; each entry selects its line.
  function report(root) {
    const form = root.closest?.("form.editor");
    const status = form?.querySelector(".diagram-status");
    if (!status) return;
    const boxes = [...root.querySelectorAll(".diagram-error")];
    status.replaceChildren();
    status.hidden = boxes.length === 0;
    if (status.hidden) return;
    status.append(boxes.length === 1 ? "Diagram error on " : "Diagram errors on ");
    boxes.forEach((box, i) => {
      if (i > 0) status.append(", ");
      const b = document.createElement("button");
      b.type = "button";
      b.className = "link";
      b.textContent = box.dataset.line ? `line ${box.dataset.line}` : `diagram ${i + 1}`;
      b.onclick = () => {
        box.scrollIntoView({ block: "nearest" });
        if (box.dataset.line) select(form.querySelector("textarea"), Number(box.dataset.line));
      };
      status.append(b);
    });
  }

  function select(textarea, line) {
    const lines = textarea.value.split("\n");
    const start = lines.slice(0, line - 1).reduce((n, l) => n + l.length + 1, 0);
    textarea.focus();
    textarea.setSelectionRange(start, start + (lines[line - 1]?.length ?? 0));
    const style = getComputedStyle(textarea);
    const height = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2;
    textarea.scrollTop = Math.max(0, (line - 1) * height - textarea.clientHeight / 3);
  }

  async function render(root) {
    const nodes = [...root.querySelectorAll("pre.mermaid:not([data-processed])")];
    if (nodes.length === 0) return report(root);
    nodes.forEach((n) => (n.dataset.processed = ""));
    let m;
    try {
      m = await load();
    } catch (e) {
      console.error("mermaid", e);
      return;
    }
    const before = lastGood.get(root);
    const previous = before?.length === nodes.length ? before : [];
    const good = [];
    for (const [i, node] of nodes.entries()) {
      const source = node.textContent;
      try {
        node.innerHTML = good[i] = await svgFor(m, source);
      } catch (e) {
        good[i] = previous[i];
        errorBlock(node, source, e, previous[i]);
      }
    }
    lastGood.set(root, good);
    report(root);
  }

  document.addEventListener("DOMContentLoaded", () => render(document));
  document.addEventListener("htmx:afterSettle", (e) => render(e.target));
})();
