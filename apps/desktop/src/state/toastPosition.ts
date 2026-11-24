import { usePref } from "./sidebarPrefs";

export const TOAST_POSITION_PREF = "rustcode.toasts.position";

export type ToastPosition = "bottom-right" | "bottom-left" | "top-right" | "top-center";

export const TOAST_POSITIONS: readonly { id: ToastPosition; label: string }[] = [
  { id: "bottom-right", label: "Bottom right" },
  { id: "bottom-left", label: "Bottom left" },
  { id: "top-right", label: "Top right" },
  { id: "top-center", label: "Top center" },
];

export const DEFAULT_TOAST_POSITION: ToastPosition = "bottom-right";

/** A stored position, read leniently: anything unknown is the default. */
export function toastPosition(raw: unknown): ToastPosition {
  return TOAST_POSITIONS.some((p) => p.id === raw) ? (raw as ToastPosition) : DEFAULT_TOAST_POSITION;
}

/** Top positions stack downward and enter from above. */
export function toastFromTop(position: ToastPosition): boolean {
  return position.startsWith("top");
}

/** The side a card is swiped toward to dismiss it: left in the left corner, right elsewhere. */
export function toastSwipeSign(position: ToastPosition): 1 | -1 {
  return position === "bottom-left" ? -1 : 1;
}

export function useToastPosition() {
  const [raw, set] = usePref<string>(TOAST_POSITION_PREF, DEFAULT_TOAST_POSITION);
  return [toastPosition(raw), set as (next: ToastPosition) => void] as const;
}
