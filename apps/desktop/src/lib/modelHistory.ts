import { readStorage, writeStorage } from "./storage";

export interface ModelRef {
  harness: string;
  model: string;
}

export const modelKey = (choice: ModelRef): string => JSON.stringify([choice.harness, choice.model]);

export function distinctModels(choices: ModelRef[]): ModelRef[] {
  const seen = new Set<string>();
  return choices.filter((choice) => {
    const key = modelKey(choice);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function toggleFavourite(choices: ModelRef[], choice: ModelRef): ModelRef[] {
  const key = modelKey(choice);
  return choices.some((entry) => modelKey(entry) === key)
    ? choices.filter((entry) => modelKey(entry) !== key)
    : distinctModels([choice, ...choices]);
}

export function rememberModel(choices: ModelRef[], choice: ModelRef): ModelRef[] {
  return distinctModels([choice, ...choices]).slice(0, 5);
}

export function recentModels(recents: ModelRef[], favourites: ModelRef[]): ModelRef[] {
  const starred = new Set(favourites.map(modelKey));
  return distinctModels(recents)
    .slice(0, 5)
    .filter((choice) => !starred.has(modelKey(choice)));
}

export function readModelHistory(kind: "favourites" | "recents"): ModelRef[] {
  try {
    const parsed: unknown = JSON.parse(readStorage(`rustcode.model-picker.${kind}`) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    const choices = distinctModels(
      parsed.filter(
        (entry): entry is ModelRef =>
          !!entry && typeof entry.harness === "string" && typeof entry.model === "string",
      ),
    );
    return kind === "recents" ? choices.slice(0, 5) : choices;
  } catch {
    return [];
  }
}

export function writeModelHistory(kind: "favourites" | "recents", choices: ModelRef[]): void {
  writeStorage(`rustcode.model-picker.${kind}`, JSON.stringify(choices));
}
