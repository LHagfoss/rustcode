// Native browser chrome the app replaces with its own:
// - `title` tooltips: the grey macOS tooltip appears late, can't be styled,
//   and doubles the app's own Tooltip. Each `title` becomes the element's
//   `aria-label` (when it has none), so screen readers keep the text, and is
//   removed.
// - Apple Intelligence Writing Tools and text suggestions in text fields:
//   `writingsuggestions="false"` turns them off in WebKit (macOS 15+).
// - Autocorrect, automatic capitalisation and spell checking, which every field
//   already asks for through `noTextHelpers` (ui/primitives.tsx): set here too,
//   so a field drawn without that spread is still quiet (docs/desktop.md ›
//   Native text fields).
//
// The attributes are set on a field when it is added, and not watched: React
// writes a prop it is given once, at mount, and its diff writes only what
// changed, so a re-render does not hand them back. Observing them would make
// every write below re-enter the observer for nothing, on a document that gets
// a new node per streamed token.

/** The attributes a text field carries, with the system's helpers off. */
const QUIET_TEXT: [string, string][] = [
  ["writingsuggestions", "false"],
  ["spellcheck", "false"],
  ["autocorrect", "off"],
  ["autocapitalize", "off"],
];

function quietField(el: Element) {
  if (
    !(
      el instanceof HTMLTextAreaElement ||
      el instanceof HTMLInputElement ||
      (el.hasAttribute("contenteditable") && el.getAttribute("contenteditable") !== "false")
    )
  ) {
    return;
  }
  for (const [name, value] of QUIET_TEXT) {
    if (el.getAttribute(name) !== value) el.setAttribute(name, value);
  }
}

function quiet(el: Element) {
  const title = el.getAttribute("title");
  if (title !== null) {
    if (title && !el.hasAttribute("aria-label")) el.setAttribute("aria-label", title);
    el.removeAttribute("title");
  }
  quietField(el);
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
