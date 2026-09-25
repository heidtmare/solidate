// Renders `pre.mermaid` blocks. mermaid.js loads on first use, from the URL in this
// script's `data-mermaid` attribute, and runs again after every htmx swap.
(() => {
  const src = document.currentScript.dataset.mermaid;
  let loading;

  function load() {
    loading ??= new Promise((resolve, reject) => {
      const s = document.createElement("script");
      s.src = src;
      s.onload = () => {
        const dark = matchMedia("(prefers-color-scheme: dark)").matches;
        mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "default" });
        resolve(mermaid);
      };
      s.onerror = reject;
      document.head.append(s);
    });
    return loading;
  }

  async function render(root) {
    const nodes = [...root.querySelectorAll("pre.mermaid:not([data-processed])")];
    if (nodes.length === 0) return;
    try {
      await (await load()).run({ nodes });
    } catch (e) {
      console.error("mermaid", e);
    }
  }

  document.addEventListener("DOMContentLoaded", () => render(document));
  document.addEventListener("htmx:afterSettle", (e) => render(e.target));
})();
