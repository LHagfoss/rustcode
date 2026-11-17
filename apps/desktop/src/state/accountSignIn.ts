// The add-account row and each account's sign-in in Settings › Harnesses
// (`AccountsList` in `ui/Harnesses.tsx`), as one state and one pure transition.
//
// Every account has its own sign-in: two can wait at once, each until its own
// deadline. The add row is one union, so "naming" and "adding" never pair by hand.

/** How long an account's sign-in is waited for, three seconds at a time. */
export const SIGN_IN_WAIT_MS = 5 * 60_000;

/** One account's sign-in, while the app waits for it or after it gave up. */
export type SignIn = { phase: "waiting"; deadline: number } | { phase: "gaveUp" };

/**
 * The add row: closed, open for a name, or an add on its way. An add remembers
 * whether it came from the name field (`named`) or from a found directory, and
 * which row to return to when it ends without closing it.
 */
export type AddRow =
  | { phase: "idle" }
  | { phase: "naming" }
  | { phase: "adding"; kind: "named" | "found"; returnTo: "idle" | "naming" };

export interface AccountsSignIn {
  add: AddRow;
  /** By account id; an account with no entry shows what the engine last read. */
  accounts: Record<string, SignIn>;
}

export const initialAccountsSignIn: AccountsSignIn = { add: { phase: "idle" }, accounts: {} };

export type AccountsSignInEvent =
  /** Sign in was pressed, or an add opened Terminal: wait until `now + SIGN_IN_WAIT_MS`. */
  | { type: "startSignIn"; id: string; now: number }
  /** A read found these accounts signed in. */
  | { type: "signedIn"; ids: readonly string[] }
  /** A tick: every wait past its deadline gives up. */
  | { type: "timedOut"; now: number }
  /** An added account cannot be signed in from here (no Terminal). */
  | { type: "noTerminal"; id: string }
  | { type: "openAdd" }
  | { type: "closeAdd" }
  | { type: "startAdd"; kind: "named" | "found" }
  | { type: "added" }
  | { type: "addFailed" };

function withAccounts(state: AccountsSignIn, accounts: Record<string, SignIn>): AccountsSignIn {
  return { ...state, accounts };
}

export function transitionAccountsSignIn(state: AccountsSignIn, event: AccountsSignInEvent): AccountsSignIn {
  switch (event.type) {
    case "startSignIn":
      return withAccounts(state, {
        ...state.accounts,
        [event.id]: { phase: "waiting", deadline: event.now + SIGN_IN_WAIT_MS },
      });
    case "signedIn": {
      const ids = event.ids.filter((id) => id in state.accounts);
      if (!ids.length) return state;
      const accounts = { ...state.accounts };
      for (const id of ids) delete accounts[id];
      return withAccounts(state, accounts);
    }
    case "timedOut": {
      let changed = false;
      const accounts: Record<string, SignIn> = {};
      for (const [id, signIn] of Object.entries(state.accounts)) {
        if (signIn.phase === "waiting" && event.now > signIn.deadline) {
          accounts[id] = { phase: "gaveUp" };
          changed = true;
        } else accounts[id] = signIn;
      }
      return changed ? withAccounts(state, accounts) : state;
    }
    case "noTerminal":
      return withAccounts(state, { ...state.accounts, [event.id]: { phase: "gaveUp" } });
    case "openAdd":
      return state.add.phase === "idle" ? { ...state, add: { phase: "naming" } } : state;
    case "closeAdd":
      if (state.add.phase === "naming") return { ...state, add: { phase: "idle" } };
      // Cancel during an add hides the row; the add still finishes.
      if (state.add.phase === "adding" && state.add.returnTo === "naming")
        return { ...state, add: { ...state.add, returnTo: "idle" } };
      return state;
    case "startAdd":
      if (state.add.phase === "adding") return state;
      if (event.kind === "named" && state.add.phase !== "naming") return state;
      return { ...state, add: { phase: "adding", kind: event.kind, returnTo: state.add.phase } };
    case "added":
      if (state.add.phase !== "adding") return state;
      // A named add closes the row; a found one leaves it as it was.
      return { ...state, add: { phase: state.add.kind === "named" ? "idle" : state.add.returnTo } };
    case "addFailed":
      return state.add.phase === "adding" ? { ...state, add: { phase: state.add.returnTo } } : state;
  }
}

/** Whether any account's sign-in is being waited for, so the poll runs. */
export function anyWaiting(state: AccountsSignIn): boolean {
  return Object.values(state.accounts).some((signIn) => signIn.phase === "waiting");
}

/** Whether the name field shows: open for a name, or a named add on its way that was not cancelled. */
export function addRowOpen(add: AddRow): boolean {
  return add.phase === "naming" || (add.phase === "adding" && add.returnTo === "naming");
}
