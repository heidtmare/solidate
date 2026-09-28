// Renders `pre.mermaid` blocks. mermaid.js loads on first use, from the URL in this
// script's `data-mermaid` attribute, and runs again after every htmx swap. Each block
// renders independently: one that fails is replaced by its error message and numbered
// source, with the last version that rendered at the same position, if any. Diagrams
// use the app's colour tokens and render again when the colour scheme changes.
(() => {
  const src = document.currentScript.dataset.mermaid;
  const scheme = matchMedia("(prefers-color-scheme: dark)");
  // Source -> SVG. Preview swaps re-send every diagram; unchanged ones skip rendering.
  const svgs = new Map();
  // Root element -> SVG per diagram position from its previous render.
  let lastGood = new WeakMap();
  // Rendered figure or error box -> { source, start } for re-rendering.
  const origin = new WeakMap();
  let loading;
  let seq = 0;

  // Unrendered blocks show a placeholder instead of their source (see app.css).
  document.documentElement.classList.add("diagrams");

  function config() {
    const css = getComputedStyle(document.documentElement);
    const v = (name) => css.getPropertyValue(`--${name}`).trim();
    return {
      startOnLoad: false,
      securityLevel: "strict",
      suppressErrorRendering: true,
      theme: "base",
      darkMode: scheme.matches,
      themeVariables: {
        fontFamily: getComputedStyle(document.body).fontFamily,
        background: v("bg"),
        primaryColor: v("soft"),
        primaryTextColor: v("fg"),
        primaryBorderColor: v("muted"),
        secondaryColor: v("add"),
        secondaryTextColor: v("add-fg"),
        tertiaryColor: v("bg"),
        tertiaryTextColor: v("fg"),
        lineColor: v("muted"),
        textColor: v("fg"),
        clusterBkg: v("bg"),
        clusterBorder: v("line"),
        edgeLabelBackground: v("bg"),
        noteBkgColor: v("warn-bg"),
        noteTextColor: v("fg"),
        noteBorderColor: v("warn"),
      },
    };
  }

  function load() {
    loading ??= new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = src;
      s.onload = () => {
        mermaid.initialize(config());
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
  // Unexpected-EOF errors report a line past the end; those map to the last non-blank line.
  function sourceLine(source, parsed) {
    const lines = source.split("\n");
    const front = /^-{3}\s*\n[\s\S]*?\n-{3}\s*\n+/.exec(source);
    let i = front ? front[0].split("\n").length - 1 : 0;
    let n = 0;
    let last = null;
    for (; i < lines.length; i++) {
      const l = lines[i];
      if (/^\s*%%(?!\{)./.test(l)) continue;
      if (n === 0 && /^\s*(%%\{.*\}%%)?\s*$/.test(l)) continue;
      if (++n === parsed) return i + 1;
      if (l.trim()) last = i + 1;
    }
    return last;
  }

  // The last heading before `el` in document order.
  function heading(el) {
    let found = null;
    for (const h of document.querySelectorAll("h1, h2, h3, h4, h5, h6")) {
      if (!(h.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING)) break;
      found = h;
    }
    return found;
  }

  function headingText(h) {
    if (!h) return "";
    const copy = h.cloneNode(true);
    copy.querySelectorAll(".anchor").forEach((a) => a.remove());
    return copy.textContent.trim();
  }

  function button(label, title, onclick) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "link";
    b.textContent = label;
    b.title = title;
    b.onclick = onclick;
    return b;
  }

  function flash(b, text) {
    const label = b.textContent;
    b.textContent = text;
    setTimeout(() => (b.textContent = label), 1500);
  }

  // SVG markup with the page background, so the file reads the same outside the app.
  function standalone(svg) {
    const copy = svg.cloneNode(true);
    copy.style.backgroundColor = getComputedStyle(document.documentElement).getPropertyValue("--bg").trim();
    return new XMLSerializer().serializeToString(copy);
  }

  function download(svg, name) {
    const url = URL.createObjectURL(new Blob([standalone(svg)], { type: "image/svg+xml" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${name}.svg`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  // Shows `svg` in a modal at its natural size, with zoom controls. Esc closes it.
  function expand(svg, label) {
    const dialog = document.createElement("dialog");
    dialog.className = "diagram-dialog";
    dialog.setAttribute("aria-label", label);
    const view = document.createElement("div");
    view.className = "diagram-zoom";
    const copy = svg.cloneNode(true);
    const box = svg.viewBox.baseVal;
    const width = box?.width || svg.getBoundingClientRect().width;
    const height = box?.height || svg.getBoundingClientRect().height;
    copy.removeAttribute("width");
    copy.removeAttribute("height");
    copy.style.maxWidth = "none";
    view.append(copy);

    let scale = 1;
    const level = document.createElement("span");
    level.className = "muted small";
    const zoom = (s) => {
      scale = Math.min(8, Math.max(0.1, s));
      copy.style.width = `${width * scale}px`;
      copy.style.height = `${height * scale}px`;
      level.textContent = `${Math.round(scale * 100)}%`;
    };
    const fit = () => zoom(0.95 * Math.min(view.clientWidth / width, view.clientHeight / height));

    const tools = document.createElement("div");
    tools.className = "diagram-tools";
    tools.append(
      button("−", "Zoom out (-)", () => zoom(scale / 1.25)),
      level,
      button("+", "Zoom in (+)", () => zoom(scale * 1.25)),
      button("100%", "Actual size (0)", () => zoom(1)),
      button("Fit", "Fit to window (f)", fit),
      button("Close", "Close (Esc)", () => dialog.close()),
    );
    dialog.append(tools, view);
    dialog.addEventListener("keydown", (e) => {
      if (e.key === "+" || e.key === "=") zoom(scale * 1.25);
      else if (e.key === "-") zoom(scale / 1.25);
      else if (e.key === "0") zoom(1);
      else if (e.key === "f") fit();
    });
    view.addEventListener("wheel", (e) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      zoom(scale * (e.deltaY < 0 ? 1.1 : 1 / 1.1));
    }, { passive: false });
    dialog.addEventListener("click", (e) => e.target === dialog && dialog.close());
    dialog.addEventListener("close", () => dialog.remove());
    document.body.append(dialog);
    dialog.showModal();
    zoom(1);
    if (width > view.clientWidth || height > view.clientHeight) fit();
  }

  // Replaces `node` with the rendered diagram and a toolbar: expand, source, copy, download.
  function figure(node, source, start, markup) {
    const fig = document.createElement("figure");
    fig.className = "diagram";
    if (start) fig.dataset.line = start;
    origin.set(fig, { source, start });

    const view = document.createElement("div");
    view.className = "diagram-view";
    view.innerHTML = markup;
    const svg = view.querySelector("svg");

    const h = heading(node);
    const title = headingText(h);
    const label = title ? `Diagram: ${title}` : "Diagram";
    // Diagrams with `accTitle` carry their own <title>; others get a label from the section.
    if (svg && !svg.querySelector(":scope > title")) {
      svg.setAttribute("role", "img");
      svg.setAttribute("aria-label", label);
    }

    const code = document.createElement("pre");
    code.className = "diagram-source";
    code.textContent = source;
    code.hidden = true;

    const tools = document.createElement("div");
    tools.className = "diagram-tools";
    const toggle = button("Source", "Show the Mermaid source", () => {
      code.hidden = !code.hidden;
      view.hidden = !code.hidden;
      toggle.textContent = code.hidden ? "Source" : "Diagram";
      toggle.title = code.hidden ? "Show the Mermaid source" : "Show the diagram";
    });
    if (svg) tools.append(button("Expand", "Open full size with zoom", () => expand(svg, label)));
    tools.append(toggle);
    if (navigator.clipboard) {
      const copy = button("Copy", "Copy the Mermaid source", () =>
        navigator.clipboard.writeText(source).then(
          () => flash(copy, "Copied"),
          () => flash(copy, "Copy failed"),
        ),
      );
      tools.append(copy);
    }
    if (svg) {
      const name = [h?.id, "diagram"].filter(Boolean).join("-");
      tools.append(button("SVG", "Download as SVG", () => download(svg, name)));
    }

    fig.append(tools, view, code);
    node.replaceWith(fig);
  }

  function errorBlock(node, source, start, error, previous) {
    const message = String(error?.message ?? error).trim();
    const reported = /\bline (\d+)/i.exec(message);
    const line = reported ? sourceLine(source, Number(reported[1])) : null;

    const box = document.createElement("div");
    box.className = "diagram-error";
    box.setAttribute("role", "group");
    if (start) box.dataset.line = line ? start + line - 1 : start;
    origin.set(box, { source, start });

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
      const last = document.createElement("div");
      last.className = "previous";
      last.innerHTML = previous;
      last.querySelector("svg")?.setAttribute("aria-hidden", "true");
      box.append(note, last);
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
      const label = box.dataset.line ? `line ${box.dataset.line}` : `diagram ${i + 1}`;
      status.append(
        button(label, "Select this line in the editor", () => {
          box.scrollIntoView({ block: "nearest" });
          if (box.dataset.line) select(form.querySelector("textarea"), Number(box.dataset.line));
        }),
      );
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
    const before = lastGood.get(root);
    const previous = before?.length === nodes.length ? before : [];
    const items = nodes.map((node, i) => {
      const item = { node, source: node.textContent, start: Number(node.dataset.line) || null };
      node.dataset.processed = "";
      // Until a changed diagram renders, show the one it replaces instead of the placeholder.
      if (previous[i] && !svgs.has(item.source)) {
        node.innerHTML = previous[i];
        node.dataset.state = "stale";
      }
      return item;
    });
    let m;
    try {
      m = await load();
    } catch (e) {
      console.error("mermaid", e);
      for (const { node, source } of items) {
        node.textContent = source;
        node.dataset.state = "source";
      }
      return;
    }
    const good = [];
    for (const [i, { node, source, start }] of items.entries()) {
      try {
        figure(node, source, start, (good[i] = await svgFor(m, source)));
      } catch (e) {
        good[i] = previous[i];
        errorBlock(node, source, start, e, previous[i]);
      }
    }
    lastGood.set(root, good);
    report(root);
  }

  // Re-renders every diagram with the new scheme's colours.
  async function rethemed() {
    if (!loading) return;
    const m = await loading;
    m.initialize(config());
    svgs.clear();
    lastGood = new WeakMap();
    for (const el of document.querySelectorAll(".diagram, .diagram-error")) {
      const o = origin.get(el);
      if (!o) continue;
      const pre = document.createElement("pre");
      pre.className = "mermaid";
      pre.textContent = o.source;
      if (o.start) pre.dataset.line = o.start;
      el.replaceWith(pre);
    }
    render(document);
  }

  document.addEventListener("DOMContentLoaded", () => render(document));
  document.addEventListener("htmx:afterSettle", (e) => render(e.target));
  scheme.addEventListener("change", rethemed);
})();
