// Chart.js 4.5.1 (MIT, Chart.js Contributors; node_modules/chart.js/LICENSE.md),
// loaded only when a block names `rustcode-lib:chart.js`, and inlined into
// that page. Nothing is fetched at run time: Vite bundles the file as text.

let loading: Promise<string> | null = null;

export function chartSource(): Promise<string> {
  loading ??= import("../../node_modules/chart.js/dist/chart.umd.min.js?raw").then((m) =>
    // The source map comment would point at a file the page cannot load.
    m.default.replace(/\/\/# sourceMappingURL=\S+\s*$/, ""),
  );
  return loading;
}
