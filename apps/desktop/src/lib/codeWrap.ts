const WRAP_OFFER_COLUMNS = 90;

/** Whether a code block has a line long enough to scroll sideways in the chat column. */
export function hasLongLine(code: string, columns = WRAP_OFFER_COLUMNS): boolean {
  let start = 0;
  for (;;) {
    const end = code.indexOf("\n", start);
    if ((end === -1 ? code.length : end) - start > columns) return true;
    if (end === -1) return false;
    start = end + 1;
  }
}
