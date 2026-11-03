// Native browser chrome the app replaces with its own:
// - `title` tooltips: the grey macOS tooltip appears late, can't be styled,
//   and doubles the app's own Tooltip. Each `title` becomes the element's
//   `aria-label` (when it has none), so screen readers keep the text, and is
//   removed.
// - Apple Intelligence Writing Tools and text suggestions in text fields:
//   `writingsuggestions="false"` turns them off in WebKit (macOS 15+).

function quiet(el: Element) {
  const title = el.getAttribute("title");
  if (title !== null) {
    if (title && !el.hasAttribute("aria-label")) el.setAttribute("aria-label", title);
    el.removeAttribute("title");
  }
  if (
    (el instanceof HTMLTextAreaElement ||
      el instanceof HTMLInputElement ||
      el.hasAttribute("contenteditable")) &&
    el.getAttribute("writingsuggestions") !== "false"
  ) {
    el.setAttribute("writingsuggestions", "false");
  }
}

function sweep(root: Element) {
  quiet(root);
  for (const el of root.querySelectorAll("[title], textarea, input, [contenteditable]")) quiet(el);
}

/** Starts watching the document; call once at start. */
export function quietNativeChrome(): void {
  sweep(document.documentElement);
  new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === "attributes") quiet(r.target as Element);
      else for (const node of r.addedNodes) if (node instanceof Element) sweep(node);
    }
  }).observe(document.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["title"],
  });
}
