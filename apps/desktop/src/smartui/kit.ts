// The pure parts of the kit's runtime behaviour, kept as page JavaScript so
// the same text runs in the sandboxed page and is unit-tested here (the page
// itself has no module system). `page.ts` splices `KIT_SCRIPT` into the
// runtime before the code that uses it. Keep every function self-contained:
// the page gets no helper the browser has not got.

export const KIT_SCRIPT = `
  // "1,234", "41 s", "12.5%", "-3" -> { value, unit }; null when the cell is not a
  // measurement ("n/a", "2026-11-02", a name), so a column sorts as text.
  const rcMeasure = (text) => {
    const raw = String(text == null ? "" : text).replace(/\\u2212/g, "-").trim();
    const match = /^(-?(?:\\d[\\d,]*)(?:\\.\\d+)?)\\s*([^\\d\\s.,]{0,6})?$/.exec(raw);
    if (!match) return null;
    const value = Number(match[1].replace(/,/g, ""));
    if (!isFinite(value)) return null;
    return { value, unit: (match[2] || "").toLowerCase() };
  };
  // One column's order: numbers first when both sides are numbers, else the
  // locale's collation, which puts "v1.9" before "v1.10".
  const rcCompare = (a, b, kind) => {
    if (kind !== "text") {
      const x = rcMeasure(a);
      const y = rcMeasure(b);
      if (x && y) {
        if (x.value !== y.value) return x.value < y.value ? -1 : 1;
        return x.unit < y.unit ? -1 : x.unit > y.unit ? 1 : 0;
      }
      if (x) return -1;
      if (y) return 1;
    }
    return String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: "base" });
  };
  // The site a source link is on, without "www."; "" for anything but a web link.
  const rcHost = (href) => {
    const match = /^https?:\\/\\/(?:[^@/?#]*@)?([^/:?#]+)/i.exec(String(href == null ? "" : href).trim());
    return match ? match[1].toLowerCase().replace(/^www\\./, "") : "";
  };
  // The numbers in a data attribute: "1,4,2" and "1 4 2" both give [1, 4, 2].
  const rcSparkValues = (text) => String(text == null ? "" : text)
    .split(/[\\s,]+/)
    .filter((part) => part !== "")
    .map(Number)
    .filter((value) => isFinite(value));
  // The points of an inline sparkline in a 100 x 30 box, one number per value.
  const rcSparkPoints = (values, width, height) => {
    const pad = 2;
    const min = Math.min.apply(null, values);
    const max = Math.max.apply(null, values);
    const span = max - min;
    const step = values.length > 1 ? (width - pad * 2) / (values.length - 1) : 0;
    return values
      .map((value, i) => {
        const x = pad + i * step;
        const y = span === 0 ? height / 2 : height - pad - ((value - min) / span) * (height - pad * 2);
        return Math.round(x * 10) / 10 + "," + Math.round(y * 10) / 10;
      })
      .join(" ");
  };
`;

/** The helpers in [`KIT_SCRIPT`], for the tests that exercise them directly. */
export function kitHelpers() {
  return new Function(
    `${KIT_SCRIPT}; return { rcMeasure, rcCompare, rcHost, rcSparkValues, rcSparkPoints };`,
  )() as {
    rcMeasure: (text: string) => { value: number; unit: string } | null;
    rcCompare: (a: string, b: string, kind: "number" | "text") => number;
    rcHost: (href: string | null) => string;
    rcSparkValues: (text: string) => number[];
    rcSparkPoints: (values: number[], width: number, height: number) => string;
  };
}
