/**
 * A dashboard stack's state and its floating indicator: a column of small dots at the card's
 * right edge, one per card, the shown one a pill that fills as its turn runs out. A dot shows its
 * card; the fill pauses while the stack is held (pointer, focus, edit mode).
 */

import { createContext, useContext } from "react";
import { STACK_INTERVAL_MS, WIDGET_NAMES, type WidgetId } from "../state/widgetLayout";
import { cx, Tooltip } from "./primitives";

export interface StackState {
  members: WidgetId[];
  at: number;
  /** The stack has turned at least once, so the shown card arrives with the turn's motion. */
  turned: boolean;
  paused: boolean;
  select: (index: number) => void;
  advance: () => void;
}

export const StackContext = createContext<StackState | undefined>(undefined);

export function useWidgetStack(): StackState | undefined {
  return useContext(StackContext);
}

export function StackDots() {
  const stack = useWidgetStack();
  if (!stack) return null;
  return (
    <div
      role="tablist"
      aria-orientation="vertical"
      aria-label="Cards in this stack"
      className="absolute top-1/2 right-0.5 z-10 flex -translate-y-1/2 flex-col items-center"
    >
      {stack.members.map((id, index) => {
        const current = index === stack.at;
        return (
          <Tooltip key={id} label={WIDGET_NAMES[id]}>
            <button
              type="button"
              role="tab"
              aria-selected={current}
              aria-label={`Show ${WIDGET_NAMES[id]}`}
              onClick={() => stack.select(index)}
              className="group flex w-2.5 items-center justify-center py-[2px]"
            >
              <span
                className={cx(
                  "relative block w-1 overflow-hidden rounded-full transition-[height,background-color] duration-200",
                  current ? "h-3 bg-fill-strong" : "h-1 bg-fg-faint/60 group-hover:bg-fg-muted",
                )}
              >
                {current && (
                  <span
                    key={stack.at}
                    className="rc-stack-progress absolute inset-x-0 top-0 block bg-accent"
                    style={{
                      animationDuration: `${STACK_INTERVAL_MS}ms`,
                      animationPlayState: stack.paused ? "paused" : "running",
                    }}
                    onAnimationEnd={stack.advance}
                  />
                )}
              </span>
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}
