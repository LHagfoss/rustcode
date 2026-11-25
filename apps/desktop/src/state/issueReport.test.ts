import { describe, expect, test } from "bun:test";
import {
  REPORT_CHECKING,
  type ReportEvent,
  type ReportState,
  reportError,
  reportIssue,
  reportOptions,
  transitionReport,
} from "./issueReport";

const options = { labels: ["bug"], can_label: true };
const run = (events: ReportEvent[], from: ReportState = REPORT_CHECKING) =>
  events.reduce(transitionReport, from);

describe("the issue report state machine", () => {
  test("the check ends ready or unavailable", () => {
    const ready = run([{ type: "options", options }]);
    expect(ready).toEqual({ phase: "ready", options });
    expect(reportOptions(ready)).toEqual(options);
    const unavailable = run([{ type: "unavailable", error: "gh is not signed in" }]);
    expect(reportError(unavailable)).toBe("gh is not signed in");
    expect(reportOptions(unavailable)).toBeNull();
  });

  test("an answer to a check that is over is dropped", () => {
    const ready = run([{ type: "options", options }]);
    expect(transitionReport(ready, { type: "unavailable", error: "late" })).toBe(ready);
    expect(transitionReport(ready, { type: "options", options: { labels: [], can_label: false } })).toBe(
      ready,
    );
  });

  test("a submit needs the check, and one runs at a time", () => {
    expect(transitionReport(REPORT_CHECKING, { type: "submit" })).toBe(REPORT_CHECKING);
    const unavailable = run([{ type: "unavailable", error: "no gh" }]);
    expect(transitionReport(unavailable, { type: "submit" })).toBe(unavailable);
    const submitting = run([{ type: "options", options }, { type: "submit" }]);
    expect(submitting.phase).toBe("submitting");
    expect(transitionReport(submitting, { type: "submit" })).toBe(submitting);
  });

  test("a filed report holds its issue until the dialog is dismissed", () => {
    const filed = run([
      { type: "options", options },
      { type: "submit" },
      { type: "filed", url: "https://x/1" },
    ]);
    expect(reportIssue(filed)).toBe("https://x/1");
    expect(reportError(filed)).toBeNull();
    expect(transitionReport(filed, { type: "check" })).toBe(filed);
    expect(transitionReport(filed, { type: "dismiss" })).toEqual(REPORT_CHECKING);
  });

  test("a rejected submit says why and can be sent again", () => {
    const rejected = run([
      { type: "options", options },
      { type: "submit" },
      { type: "rejected", error: "HTTP 422" },
    ]);
    expect(reportError(rejected)).toBe("HTTP 422");
    expect(reportOptions(rejected)).toEqual(options);
    const again = transitionReport(rejected, { type: "submit" });
    expect(again).toEqual({ phase: "submitting", options });
    expect(reportError(again)).toBeNull();
  });

  test("opening again or Retry never shows an earlier submit failure", () => {
    const rejected = run([
      { type: "options", options },
      { type: "submit" },
      { type: "rejected", error: "HTTP 422" },
    ]);
    const reopened = transitionReport(rejected, { type: "check" });
    expect(reportError(reopened)).toBeNull();
    const failedCheck = transitionReport(reopened, { type: "unavailable", error: "no gh" });
    expect(reportError(failedCheck)).toBe("no gh");
    expect(reportError(transitionReport(failedCheck, { type: "check" }))).toBeNull();
  });

  test("a check or a dismiss cannot interrupt a submit", () => {
    const submitting = run([{ type: "options", options }, { type: "submit" }]);
    expect(transitionReport(submitting, { type: "check" })).toBe(submitting);
    expect(transitionReport(submitting, { type: "dismiss" })).toBe(submitting);
  });
});
