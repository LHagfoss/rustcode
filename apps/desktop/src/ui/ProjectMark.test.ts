// A project's mark: the initials it shows, and the tint its name picks.

import { describe, expect, test } from "bun:test";
import { projectInitials, projectTint } from "./ProjectMark";

describe("a project's mark", () => {
  test("two words give their first letters", () => {
    expect(projectInitials("Rust Code")).toBe("RC");
    expect(projectInitials("my-side project")).toBe("MS");
  });

  test("one word gives its ends, so the name is not only its first two letters", () => {
    expect(projectInitials("rustcode")).toBe("RE");
    expect(projectInitials("website")).toBe("WE");
  });

  test("a short name is itself", () => {
    expect(projectInitials("ui")).toBe("UI");
    expect(projectInitials("x")).toBe("X");
  });

  test("a name with nothing in it still draws something", () => {
    expect(projectInitials("")).toBe("?");
    expect(projectInitials("/")).toBe("?");
  });

  test("a name keeps its tint, and two names differ", () => {
    expect(projectTint("rustcode")).toBe(projectTint("rustcode"));
    expect(projectTint("rustcode")).not.toBe(projectTint("website"));
  });
});
