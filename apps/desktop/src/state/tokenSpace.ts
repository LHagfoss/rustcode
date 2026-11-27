/** A picked token already ends in a space: one more typed right after it is not doubled. */
export function doublesTokenSpace(before: string, after: string, tokenEnd: number | null): boolean {
  if (tokenEnd === null || before[tokenEnd - 1] !== " ") return false;
  return after === `${before.slice(0, tokenEnd)} ${before.slice(tokenEnd)}`;
}
