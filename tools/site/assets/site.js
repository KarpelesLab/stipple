// The site's few behaviors: the guides menu on narrow screens, and copying
// code blocks.
document.addEventListener("DOMContentLoaded", () => {
  const toggle = document.querySelector(".menu-toggle");
  const sidebar = document.getElementById("sidebar");
  if (toggle && sidebar) {
    toggle.addEventListener("click", () => {
      const open = document.body.classList.toggle("nav-open");
      toggle.setAttribute("aria-expanded", String(open));
    });
    sidebar.addEventListener("click", (e) => {
      if (e.target.closest("a")) document.body.classList.remove("nav-open");
    });
    // Keep the current page in view in a long sidebar.
    const here = sidebar.querySelector('[aria-current="page"]');
    if (here) sidebar.scrollTop = here.offsetTop - sidebar.clientHeight / 2;
  }
  // A live example narrower than it was made for (a phone) runs at its own
  // size, scaled down to fit, rather than squeezed.
  const fit = (frame) => {
    const iframe = frame.querySelector("iframe");
    const style = getComputedStyle(frame.parentNode);
    const w = parseFloat(style.getPropertyValue("--w"));
    const h = parseFloat(style.getPropertyValue("--h"));
    const avail = frame.parentNode.clientWidth;
    if (!w || !h) return;
    if (avail < w * 0.9) {
      const s = avail / w;
      Object.assign(iframe.style, { width: `${w}px`, height: `${h}px`, transform: `scale(${s})`, transformOrigin: "0 0" });
      frame.style.height = `${h * s}px`;
    } else {
      Object.assign(iframe.style, { width: "", height: "", transform: "" });
      frame.style.height = "";
    }
  };
  const frames = document.querySelectorAll(".live-frame");
  const ro = new ResizeObserver(() => frames.forEach(fit));
  frames.forEach((f) => ro.observe(f.parentNode));
  for (const pre of document.querySelectorAll(".prose pre")) {
    const button = document.createElement("button");
    button.className = "copy";
    button.type = "button";
    button.textContent = "Copy";
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(pre.querySelector("code")?.innerText ?? pre.innerText);
        button.textContent = "Copied";
      } catch (_) {
        button.textContent = "Press Ctrl+C";
      }
      setTimeout(() => (button.textContent = "Copy"), 1500);
    });
    pre.append(button);
  }
});
