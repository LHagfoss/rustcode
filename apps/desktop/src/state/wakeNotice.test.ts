import { describe, expect, test } from "bun:test";
import { harnessId, wakeNotice } from "./wakeNotice";

const line = (over: string) =>
  `- link_id a/${over} · done · “${over}” · Pi · worked 3m 49s · session s-${over} · 2 files changed · 3s ago`;

describe("a wake row's divider", () => {
  test("a finished delegate keeps its title, harness and worked time", () => {
    const text = [
      "Delegate finished: Fix composer focus",
      "",
      "Finished:",
      "",
      line("b"),
      "  answer: it is done",
      "",
      "The rest is in read_session …",
      "<!-- rustcode:wake -->",
    ].join("\n");
    expect(wakeNotice(text)).toMatchObject({
      label: "Delegate finished",
      titles: "b",
      harness: "Pi",
      worked: "3m 49s",
      failed: false,
      asking: [],
    });
    expect(wakeNotice(text)?.finished.map((d) => d.state)).toEqual(["done"]);
  });

  test("several delegates show the longest time and every title", () => {
    const text = [
      "2 delegates finished",
      "",
      "Finished:",
      line("b"),
      line("c").replace("worked 3m 49s", "worked 8m 2s"),
    ].join("\n");
    const notice = wakeNotice(text)!;
    expect(notice.label).toBe("2 delegates finished");
    expect(notice.titles).toBe("b, c");
    expect(notice.worked).toBe("8m 2s");
    // Several show their marks, not a single harness name.
    expect(notice.harness).toBeUndefined();
  });

  test("a failed delegate says so in words, and mixed results count the failures", () => {
    const failed = wakeNotice(
      [
        "Delegate failed: Review PR 3110",
        "",
        "Finished:",
        line("b").replace(" · done ·", " · failed ·"),
      ].join("\n"),
    )!;
    expect(failed.label).toBe("Delegate failed");
    expect(failed.failed).toBe(true);
    const mixed = wakeNotice(
      [
        "2 delegates finished · 1 failed",
        "",
        "Finished:",
        line("b"),
        line("c").replace(" · done ·", " · failed ·"),
      ].join("\n"),
    )!;
    expect(mixed.label).toBe("2 delegates finished · 1 failed");
    expect(mixed.failed).toBe(true);
    const all = wakeNotice(["2 delegates failed", "", "Finished:", line("b"), line("c")].join("\n"))!;
    expect(all.label).toBe("2 delegates failed");
  });

  test("an ask keeps the same words as a finish", () => {
    const text = [
      "Delegate asks: Answer the question",
      "",
      "Waiting on you (the user answers these; you cannot):",
      "",
      line("b").replace(" · done ·", " · needs_user ·"),
    ].join("\n");
    const notice = wakeNotice(text)!;
    expect(notice.label).toBe("Delegate asks");
    expect(notice.titles).toBe("b");
    expect(notice.finished).toEqual([]);
    expect(notice.asking.map((d) => d.state)).toEqual(["needs_user"]);
  });

  test("an old row still reads with what its lines carry, and no made-up time", () => {
    const text = [
      "1 child finished",
      "",
      "Finished:",
      "",
      "- link_id a/old · done · “Review delegate wake changes” · session s-old · 2 files changed",
    ].join("\n");
    expect(wakeNotice(text)).toMatchObject({
      label: "Delegate finished",
      titles: "Review delegate wake changes",
      harness: undefined,
      worked: undefined,
      failed: false,
    });
    // The barest wake an older build wrote: the divider is still sensible.
    expect(wakeNotice("1 child finished\n\nFinished:")).toMatchObject({ label: "Delegate finished" });
    expect(wakeNotice("1 child finished · 1 needs you")).toMatchObject({
      label: "Delegate finished · 1 delegate asks",
    });
  });

  test("a message that is not a wake makes no divider", () => {
    expect(wakeNotice("Please review the changes")).toBeNull();
    expect(wakeNotice("")).toBeNull();
  });

  test("a harness name becomes the id its mark reads", () => {
    expect(harnessId("Pi")).toBe("pi");
    expect(harnessId("Claude Code")).toBe("claude-code");
    expect(harnessId("RustCode")).toBe("rustcode");
  });
});
