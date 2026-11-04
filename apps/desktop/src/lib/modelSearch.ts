// The model picker's search: a fuzzy match over a model's name and its id,
// scored so a substring ("sonnet") and a squeezed subsequence ("c5sn") both
// find it, with the nearest one first. The lists can hold hundreds of models,
// so the caller shows a window and says how many wait behind it.

export type Matchable = { name: string; id?: string };

/** How well `text` answers `query`, or null when it does not: higher is closer. */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.trim().toLowerCase().replace(/\s+/g, " ");
  if (!q) return 0;
  const t = text.toLowerCase();
  if (!t) return null;
  const direct = t.indexOf(q);
  const letters = q.replace(/ /g, "");
  // Every letter, in order: no match without that.
  let at = 0;
  let run = 0;
  let sub = 0;
  for (const ch of letters) {
    const found = t.indexOf(ch, at);
    if (found < 0) return direct >= 0 ? 100 - direct : null;
    run = found === at ? run + 1 : 1;
    sub += run * 2;
    at = found + 1;
  }
  // A hit on the whole query beats letters scattered through the name; a hit at
  // the start beats one later, and a short field beats a long one.
  if (direct >= 0) return 100 - direct + sub * 0.1;
  return sub * 0.5 - t.length * 0.01;
}

/**
 * The models that answer `query`, nearest first, at most `limit` of them, and
 * how many matched in all so the list can say the rest wait behind it.
 */
export function matchModels<T extends Matchable>(
  query: string,
  models: readonly T[],
  limit = 60,
): { shown: T[]; total: number } {
  if (!query.trim()) return { shown: models.slice(0, limit), total: models.length };
  const scored: { model: T; at: number; score: number }[] = [];
  models.forEach((model, at) => {
    const name = fuzzyScore(query, model.name);
    const id = model.id ? fuzzyScore(query, model.id) : null;
    const score = name === null && id === null ? null : Math.max(name ?? Number.NEGATIVE_INFINITY, id ?? 0);
    if (score !== null) scored.push({ model, at, score });
  });
  scored.sort((a, b) => b.score - a.score || a.at - b.at);
  return { shown: scored.slice(0, limit).map((hit) => hit.model), total: scored.length };
}
