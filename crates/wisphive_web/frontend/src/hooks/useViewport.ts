import { useSyncExternalStore } from "react";

// Single source of truth for the JS-side mobile breakpoint (itr#487). CSS
// media queries cannot read JS constants, so the same number is duplicated in
// app.css (`@media (max-width: 900px)`) — change both together. Other
// features that need breakpoint-conditional behavior should import from here
// rather than hand-rolling their own matchMedia query.
//
// Two-tier reality: this is the *pane-reflow* breakpoint (where side-by-side
// layouts like the Terminals two-step workflow collapse to one pane at a
// time). The app shell's compact chrome (sidebar→top-bar etc.) kicks in
// lower, at app.css's 768px tier — so 769-900px is "mobile" here while the
// shell still wears desktop chrome. That is intentional: pane splits run out
// of room before the nav does.
export const MOBILE_BREAKPOINT_PX = 900;

const MOBILE_QUERY = `(max-width: ${MOBILE_BREAKPOINT_PX}px)`;

/** One-shot breakpoint probe — safe anywhere, including outside React and in
 * jsdom (which lacks matchMedia unless a test stubs it). */
export function isMobileViewport(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia(MOBILE_QUERY).matches;
}

/** Mirror the breakpoint onto :root so plain CSS (and devtools) can key on it:
 * `--is-mobile: 0|1` plus `data-viewport="mobile|desktop"` for selector use
 * (`:root[data-viewport="mobile"] .foo { … }`). */
function syncRootFlags(mobile: boolean) {
  const root = document.documentElement;
  root.style.setProperty("--is-mobile", mobile ? "1" : "0");
  root.dataset.viewport = mobile ? "mobile" : "desktop";
}

function subscribe(onChange: () => void): () => void {
  if (typeof window.matchMedia !== "function") return () => {};
  const mql = window.matchMedia(MOBILE_QUERY);
  const handler = () => {
    syncRootFlags(mql.matches);
    onChange();
  };
  if (typeof mql.addEventListener === "function") {
    mql.addEventListener("change", handler);
    return () => mql.removeEventListener("change", handler);
  }
  // Safari < 14 (and some test stubs) only expose the legacy listener API.
  if (typeof mql.addListener === "function") {
    mql.addListener(handler);
    return () => mql.removeListener(handler);
  }
  return () => {};
}

/** Reactive breakpoint hook. Any mounted subscriber also keeps the :root
 * flags in sync; App.tsx holds one for the app's lifetime. */
export function useIsMobile(): boolean {
  return useSyncExternalStore(subscribe, isMobileViewport);
}

// Stamp the flags once at load so CSS keyed on :root is correct before any
// component subscribes (guarded: jsdom import must not throw).
if (typeof window !== "undefined" && typeof document !== "undefined") {
  syncRootFlags(isMobileViewport());
}
