import { useEffect } from "react";

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
  onSpawn?: () => void;
  onHelp?: () => void;
}

export function useKeyboard(actions: KeyboardActions) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Don't intercept when typing in inputs
      const tag = (e.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;

      // Don't intercept when modals are open
      if (document.querySelector(".modal-overlay")) {
        if (e.key === "Escape" && actions.onBack) {
          actions.onBack();
        }
        return;
      }

      switch (e.key) {
        // Navigation
        case "j":
        case "ArrowDown":
          e.preventDefault();
          actions.onNext?.();
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          actions.onPrev?.();
          break;
        case "Enter":
          actions.onSelect?.();
          break;
        case "Escape":
          actions.onBack?.();
          break;

        // Actions
        case "y":
          actions.onApprove?.();
          break;
        case "n":
          actions.onDeny?.();
          break;

        // View switching (only lowercase, not in inputs)
        case "1":
          actions.onViewQueue?.();
          break;
        case "2":
          actions.onViewHistory?.();
          break;
        case "3":
          actions.onViewSessions?.();
          break;
        case "4":
          actions.onViewProjects?.();
          break;
        case "5":
          actions.onViewAgents?.();
          break;
        case "6":
          actions.onViewConfig?.();
          break;

        // Spawn
        case "N":
          actions.onSpawn?.();
          break;

        // Help
        case "?":
          actions.onHelp?.();
          break;
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [actions]);
}
