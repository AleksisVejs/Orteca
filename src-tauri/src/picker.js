// Runs in every frame of the app window; does nothing unless Orteca's preview
// tab (the parent) asks. Lets the user click one element and reports it back.
(() => {
  if (window.top === window) return;
  let on = false;
  let box = null;

  const short = (s, n) => (s.length > n ? s.slice(0, n) + "…" : s);

  function selector(el) {
    const parts = [];
    for (; el && el.nodeType === 1 && el !== document.documentElement; el = el.parentElement) {
      let part = el.localName;
      if (el.id) {
        parts.unshift(`${part}#${CSS.escape(el.id)}`);
        break;
      }
      const same = el.parentElement ? [...el.parentElement.children].filter((c) => c.localName === part) : [];
      if (same.length > 1) part += `:nth-of-type(${same.indexOf(el) + 1})`;
      parts.unshift(part);
    }
    return parts.join(" > ");
  }

  // Dev builds only: Vue keeps the component's file, React 18 and older the
  // line. Walk up so a plain <div> still names the component that rendered it.
  function source(el) {
    for (; el; el = el.parentElement) {
      const file = el.__vueParentComponent?.type?.__file;
      if (file) return file;
      const key = Object.keys(el).find((k) => k.startsWith("__reactFiber$"));
      for (let f = key && el[key]; f; f = f.return) {
        if (f._debugSource) return `${f._debugSource.fileName}:${f._debugSource.lineNumber}`;
      }
    }
    return "";
  }

  function outline(el) {
    if (!box) {
      box = document.createElement("div");
      box.style.cssText = "position:fixed;z-index:2147483647;pointer-events:none;outline:2px solid #4f8cff;background:#4f8cff22";
      document.documentElement.appendChild(box);
    }
    const r = el.getBoundingClientRect();
    Object.assign(box.style, { left: r.left + "px", top: r.top + "px", width: r.width + "px", height: r.height + "px", display: "block" });
  }

  function stop() {
    on = false;
    if (box) box.style.display = "none";
    document.documentElement.style.cursor = "";
  }

  const over = (e) => on && outline(e.target);
  const click = (e) => {
    if (!on) return;
    e.preventDefault();
    e.stopPropagation();
    const el = e.target;
    stop();
    parent.postMessage(
      {
        orteca: "picked",
        url: location.href,
        selector: selector(el),
        file: source(el),
        id: el.id,
        cls: el.getAttribute("class") || "",
        tag: el.localName,
        text: short((el.innerText || "").trim().replace(/\s+/g, " "), 120),
        html: short(el.outerHTML, 500),
      },
      "*",
    );
  };

  document.addEventListener("mouseover", over, true);
  document.addEventListener("click", click, true);
  document.addEventListener("keydown", (e) => on && e.key === "Escape" && (stop(), parent.postMessage({ orteca: "cancelled" }, "*")), true);
  window.addEventListener("message", (e) => {
    if (e.source !== parent || !e.data || e.data.orteca !== "pick") return;
    on = !!e.data.on;
    document.documentElement.style.cursor = on ? "crosshair" : "";
    if (!on) stop();
  });
})();
