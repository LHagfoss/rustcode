// The composer's field grows to its draft and shrinks back. Reading the field itself cannot say
// how tall its text is while its height is animating: `scrollHeight` never reports less than the
// box, so a box caught mid-transition reports the height it is leaving, and the field then never
// shrinks. The draft is measured on a twin of the field instead — same font, leading, padding and
// width, no transition — and the field is given the final height once, so its own transition runs
// from the height it had to the height it needs.

/** The field, as the measurement needs it. */
export interface FieldText {
  /** The draft, exactly as the field holds it. */
  value: string;
  /** The field's padding box, scrollbar excluded: the width its text wraps at. */
  width: number;
}

/** The measuring twin: a textarea nobody sees, with no transition of its own. */
export interface FieldGauge {
  value: string;
  scrollHeight: number;
  style: { width: string; height: string };
}

/**
 * The height the field takes for `field`'s text, between whole lines (`min`) and the point it
 * starts scrolling (`max`). The twin is measured at zero height, so the number is the text plus the
 * field's own padding and nothing the field's current box adds.
 */
export function measuredFieldHeight(gauge: FieldGauge, field: FieldText, min: number, max: number): number {
  gauge.style.width = `${field.width}px`;
  gauge.style.height = "0px";
  gauge.value = field.value;
  return Math.min(Math.max(gauge.scrollHeight, min), max);
}
