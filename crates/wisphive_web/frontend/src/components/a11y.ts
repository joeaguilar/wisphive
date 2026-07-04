import type { KeyboardEvent } from "react";

/**
 * Props that make a non-`<button>` element (a clickable row / card / header)
 * operable by keyboard and exposed to assistive tech as a button:
 * `role="button"`, `tabIndex=0`, and Enter/Space activation.
 *
 * The keydown only fires when the event originates on the element itself
 * (`target === currentTarget`), so a nested real `<button>` inside the row
 * keeps its own Enter/Space behaviour instead of also triggering the row.
 * Pair with the element's existing `onClick` (which mouse users still use);
 * nested action buttons should `stopPropagation` on their own click.
 */
export function activate(onActivate: () => void) {
  return {
    role: "button" as const,
    tabIndex: 0,
    onKeyDown: (e: KeyboardEvent) => {
      if (e.target !== e.currentTarget) return;
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}
