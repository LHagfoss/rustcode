// A count with its noun in agreement: "1 turn", "2 turns". One helper, so a
// row that says "1 turns" cannot be written by accident.

/** `n` and its noun, singular only at exactly one. */
export function count(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
