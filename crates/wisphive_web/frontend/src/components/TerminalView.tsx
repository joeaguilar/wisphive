import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { TerminalSessionMeta } from "../types/protocol";

interface Props {
  session: TerminalSessionMeta;
  replayMode: boolean;
  onInput: (id: string, data: string) => void;
  onResize: (id: string, cols: number, rows: number) => void;
  registerHandler: (
    id: string,
    handler: (id: string, direction: "chunk" | "catchup" | "replay_chunk", bytes: Uint8Array) => void,
  ) => () => void;
}

export function TerminalView({ session, replayMode, onInput, onResize, registerHandler }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    // xterm renders to a canvas and can't resolve CSS custom properties, so
    // read the canvas colour from the --bg design token at mount instead of
    // hard-coding it (keeps the terminal in step with the theme).
    const terminalBg =
      getComputedStyle(document.documentElement).getPropertyValue("--bg").trim() || "#0a0a0a";

    const term = new Terminal({
      cols: session.cols,
      rows: session.rows,
      fontFamily: "Menlo, Monaco, Consolas, monospace",
      fontSize: 13,
      theme: { background: terminalBg },
      cursorBlink: !replayMode,
      disableStdin: replayMode,
      scrollback: 5000,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(containerRef.current);
    term.focus();

    termRef.current = term;
    fitRef.current = fit;

    // Flex layout isn't guaranteed to be settled in the same tick as mount.
    // Fit once synchronously so we have a best-effort size, then again in rAF
    // so the viewport matches the final container height — otherwise the
    // scrollable area can end up mis-sized until a window resize forces a
    // recompute. Push the fitted dims to the daemon so the PTY matches.
    const syncFit = () => {
      try {
        fit.fit();
        if (!replayMode) {
          onResize(session.id, term.cols, term.rows);
        }
      } catch {
        // fit() can throw if the container is detached — safe to ignore.
      }
    };
    syncFit();
    const rafId = requestAnimationFrame(syncFit);

    // Feed incoming PTY bytes into xterm.
    const unregister = registerHandler(session.id, (_id, direction, bytes) => {
      // Catchup replaces prior screen state by issuing a reset first.
      if (direction === "catchup") {
        term.reset();
      }
      term.write(bytes);
    });

    // Forward keyboard input (skip in replay mode).
    const inputDisposable = term.onData((data) => {
      if (!replayMode) {
        onInput(session.id, data);
      }
    });

    // Forward resize events to the daemon so the PTY reshapes.
    const resizeObserver = new ResizeObserver(() => {
      if (fitRef.current && termRef.current) {
        try {
          fitRef.current.fit();
          if (!replayMode) {
            onResize(session.id, termRef.current.cols, termRef.current.rows);
          }
        } catch {
          // fit() can throw during unmount — safe to ignore.
        }
      }
    });
    resizeObserver.observe(containerRef.current);

    return () => {
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
      inputDisposable.dispose();
      unregister();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.id, replayMode]);

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={{ padding: 6, fontSize: 12, background: "var(--bg-sidebar)" }}>
        <strong>{session.label ?? session.id.slice(0, 8)}</strong>
        {" · "}
        {session.command} {session.args.join(" ")}
        {" · "}
        <span className={`term-status term-status-${session.status}`}>{session.status}</span>
        {replayMode && <span style={{ marginLeft: 8, color: "var(--yellow)" }}>(replay)</span>}
      </div>
      <div
        ref={containerRef}
        style={{ flex: 1, minHeight: 0, background: "var(--bg)" }}
      />
    </div>
  );
}
