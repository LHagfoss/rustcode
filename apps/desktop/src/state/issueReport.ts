/**
 * The issue report's way from the GitHub check to a filed issue, as one value (issue 4338).
 * `transitionReport` is the one way between phases, and each phase carries only its own data:
 *
 * - `checking`: the GitHub CLI's setup is being read.
 * - `unavailable`: it could not be read; Retry checks again.
 * - `ready`: the report can be submitted. `error` is why the last submit failed.
 * - `submitting`: the report is out; the dialog cannot close.
 * - `submitted`: the issue exists at `url`.
 */

export type ReportOptions = { labels: string[]; can_label: boolean };

export type ReportState =
  | { phase: "checking" }
  | { phase: "unavailable"; error: string }
  | { phase: "ready"; options: ReportOptions; error?: string }
  | { phase: "submitting"; options: ReportOptions }
  | { phase: "submitted"; url: string };

export type ReportEvent =
  /** The dialog opened, or Retry was pressed. */
  | { type: "check" }
  | { type: "options"; options: ReportOptions }
  | { type: "unavailable"; error: string }
  | { type: "submit" }
  | { type: "filed"; url: string }
  | { type: "rejected"; error: string }
  /** The dialog closed over a filed issue: the next open starts a new report. */
  | { type: "dismiss" };

export const REPORT_CHECKING: ReportState = { phase: "checking" };

/** The pure step between report phases; an event that does not apply answers `state` itself. */
export function transitionReport(state: ReportState, event: ReportEvent): ReportState {
  switch (event.type) {
    case "check":
      return state.phase === "ready" || state.phase === "unavailable" ? REPORT_CHECKING : state;
    case "options":
      return state.phase === "checking" ? { phase: "ready", options: event.options } : state;
    case "unavailable":
      return state.phase === "checking" ? { phase: "unavailable", error: event.error } : state;
    case "submit":
      return state.phase === "ready" ? { phase: "submitting", options: state.options } : state;
    case "filed":
      return state.phase === "submitting" ? { phase: "submitted", url: event.url } : state;
    case "rejected":
      return state.phase === "submitting"
        ? { phase: "ready", options: state.options, error: event.error }
        : state;
    case "dismiss":
      return state.phase === "submitted" ? REPORT_CHECKING : state;
  }
}

export const reportOptions = (state: ReportState): ReportOptions | null =>
  state.phase === "ready" || state.phase === "submitting" ? state.options : null;

/** What the dialog's alert says: the failed check, or the failed submit. */
export const reportError = (state: ReportState): string | null =>
  state.phase === "unavailable" || state.phase === "ready" ? (state.error ?? null) : null;

export const reportIssue = (state: ReportState): string | null =>
  state.phase === "submitted" ? state.url : null;
