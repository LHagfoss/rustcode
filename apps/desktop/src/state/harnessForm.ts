// The "Add harness" form for a local ACP command, as pure logic: what is wrong
// with the entries, and the request they make. The command runs directly, one
// argument per row, never through a shell (docs/harnesses.md).

export interface EnvRow {
  name: string;
  value: string;
}

export interface HarnessForm {
  name: string;
  /** The executable's name or path, nothing else. */
  executable: string;
  /** One literal argument per entry; a blank row is dropped. */
  args: string[];
  env: EnvRow[];
}

export interface HarnessFormErrors {
  name?: string;
  executable?: string;
  /** By row; undefined for a row that is fine. */
  args: (string | undefined)[];
  env: ({ name?: string; value?: string } | undefined)[];
}

export interface HarnessRequest {
  id: string;
  name: string;
  command: string[];
  env: Record<string, string>;
}

export const EMPTY_HARNESS_FORM: HarnessForm = { name: "", executable: "", args: [], env: [] };

const NAME_MAX = 64;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** A path, where spaces belong to the name: absolute, home-relative, relative or a drive letter. */
function looksLikePath(text: string): boolean {
  return /^([/\\]|~[/\\]|\.{1,2}[/\\]|[A-Za-z]:[/\\])/.test(text);
}

/** The arguments that will be passed: blank rows dropped, the rest exactly as typed. */
export function commandArgs(form: HarnessForm): string[] {
  return form.args.filter((arg) => arg.length > 0);
}

/** The id a harness named `name` gets: lowercase letters, digits, `-`, `_` and `.`, and free among `taken`. */
export function harnessIdFor(name: string, taken: readonly string[]): string {
  const base =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_.-]+/g, "-")
      .replace(/^[-.]+|[-.]+$/g, "")
      .slice(0, 56) || "custom";
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) {
    const id = `${base}-${n}`;
    if (!taken.includes(id)) return id;
  }
}

/** What is wrong with `form`; `existingNames` are the harnesses already listed. */
export function validateHarnessForm(
  form: HarnessForm,
  existingNames: readonly string[] = [],
): HarnessFormErrors {
  const errors: HarnessFormErrors = { args: [], env: [] };
  const name = form.name.trim();
  if (!name) errors.name = "Give it a name.";
  else if (name.length > NAME_MAX) errors.name = `Use at most ${NAME_MAX} characters.`;
  else if (existingNames.some((existing) => existing.trim().toLowerCase() === name.toLowerCase()))
    errors.name = "A harness already has this name.";

  const executable = form.executable.trim();
  if (!executable) errors.executable = "Enter the executable's name or path.";
  else if (executable.includes("\0")) errors.executable = "The executable cannot contain a NUL character.";
  else if (/\s/.test(executable) && !looksLikePath(executable))
    errors.executable = "Only the executable goes here; add each argument as its own row.";
  else if (/["']/.test(executable) && !looksLikePath(executable))
    errors.executable = "No quotes: the command is not run by a shell.";

  errors.args = form.args.map((arg) =>
    arg.includes("\0") ? "An argument cannot contain a NUL character." : undefined,
  );

  const seen = new Set<string>();
  errors.env = form.env.map((row) => {
    const name = row.name.trim();
    // A row with nothing in it is dropped, like a blank argument.
    if (!name && !row.value) return undefined;
    const problem: { name?: string; value?: string } = {};
    if (!name) problem.name = "Name the variable.";
    else if (!ENV_NAME.test(name)) problem.name = "Letters, digits and _, not starting with a digit.";
    else if (seen.has(name)) problem.name = "Already set above.";
    if (row.value.includes("\0")) problem.value = "A value cannot contain a NUL character.";
    seen.add(name);
    return problem.name || problem.value ? problem : undefined;
  });
  return errors;
}

export function hasFormErrors(errors: HarnessFormErrors): boolean {
  return !!(errors.name || errors.executable || errors.args.some(Boolean) || errors.env.some(Boolean));
}

/**
 * The `_rustcode/harnesses/set` request `form` makes: the executable then each
 * argument as typed, and the variables that have a name. Call it on a valid form.
 */
export function buildHarnessRequest(form: HarnessForm, takenIds: readonly string[]): HarnessRequest {
  const name = form.name.trim();
  const env: Record<string, string> = {};
  for (const row of form.env) {
    const variable = row.name.trim();
    if (variable) env[variable] = row.value;
  }
  return {
    id: harnessIdFor(name, takenIds),
    name,
    command: [form.executable.trim(), ...commandArgs(form)],
    env,
  };
}
