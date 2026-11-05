import { describe, expect, test } from "bun:test";
import { FIELD_FADE, fieldFade } from "./fade";

describe("field fade", () => {
  test("a draft that fits fades at neither edge", () => {
    expect(fieldFade(0, 200, 200)).toBe("none");
    expect(fieldFade(0, 120, 200)).toBe("none");
  });

  test("a field at its top fades only under the last line that shows", () => {
    expect(fieldFade(0, 900, 200)).toBe("bottom");
  });

  test("a field scrolled into the middle fades at both edges", () => {
    expect(fieldFade(120, 900, 200)).toBe("both");
    expect(fieldFade(340, 900, 200)).toBe("both");
  });

  test("a field at its end fades only above the first line that shows", () => {
    expect(fieldFade(700, 900, 200)).toBe("top");
  });

  test("the very bottom still reads as the bottom when scrollTop rounds short of it", () => {
    // 900 − 200 − 699.5 = 0.5 px of text below: the last line is in view, so it is not dimmed.
    expect(fieldFade(699.5, 900, 200)).toBe("top");
    expect(fieldFade(699, 900, 200)).toBe("top");
    // A fractional height rounds the scroll range up: 699.25 leaves more than a px below.
    expect(fieldFade(697.5, 900, 200)).toBe("both");
  });

  test("text only a hair below the edge does not start a fade", () => {
    // Sub-pixel overflow (a rounding artefact of the box) is not a line to scroll to.
    expect(fieldFade(0, 200.5, 200)).toBe("none");
    expect(fieldFade(0.5, 900, 200)).toBe("bottom");
  });

  test("the fade is deep enough to soften a line but not to dim two", () => {
    expect(FIELD_FADE).toBeGreaterThanOrEqual(16);
    expect(FIELD_FADE).toBeLessThanOrEqual(24);
  });
});
