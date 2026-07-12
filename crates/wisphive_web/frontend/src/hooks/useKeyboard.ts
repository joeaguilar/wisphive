import { useEffect, useLayoutEffect, useRef } from "react";

interface KeyboardActions {
  onApprove?: () => void;
  onDeny?: () => void;
  onBack?: () => void;
  onNext?: () => void;
  onPrev?: () => void;
  onSelect?: () => void;
  onViewQueue?: () => void;
  onViewHistory?: () => void;
  onViewConfig?: () => void;
  onViewSessions?: () => void;
  onViewProjects?: () => void;
  onViewAgents?: () => void;
  onViewTerminals?: () => void;
  onSpawn?: () => void;
  onHelp?: () => void;
}

export function useKeyboard(actions: KeyboardActions) {
  const actionsRef = useRef(actions);
  useLayoutEffect(() => {
    actionsRef.current = actions;
  });

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const currentActions = actionsRef.current;

      // Don't intercept when typing in inputs
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;

      // Don't intercept when modals are open
      if (document.querySelector(".modal-overlay")) {
        if (e.key === "Escape" && currentActions.onBack) {
          currentActions.onBack();
        }
        return;
      }

      switch (e.key) {
        // Navigation
        case "j":
        case "ArrowDown":
          e.preventDefault();
          currentActions.onNext?.();
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          currentActions.onPrev?.();
          break;
        case "Enter":
          currentActions.onSelect?.();
          break;
        case "Escape":
          currentActions.onBack?.();
          break;

        // Actions
        case "y":
          currentActions.onApprove?.();
          break;
        case "n":
          currentActions.onDeny?.();
          break;

        // View switching (only lowercase, not in inputs)
        case "1":
          currentActions.onViewQueue?.();
          break;
        case "2":
          currentActions.onViewHistory?.();
          break;
        case "3":
          currentActions.onViewSessions?.();
          break;
        case "4":
          currentActions.onViewProjects?.();
          break;
        case "5":
          currentActions.onViewAgents?.();
          break;
        case "6":
          currentActions.onViewConfig?.();
          break;
        case "7":
          currentActions.onViewTerminals?.();
          break;

        // Spawn
        case "N":
          currentActions.onSpawn?.();
          break;

        // Help
        case "?":
          currentActions.onHelp?.();
          break;
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
}
