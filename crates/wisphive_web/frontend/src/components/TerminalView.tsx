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

    const term = new Terminal({
      cols: session.cols,
      rows: session.rows,
      fontFamily: "Menlo, Monaco, Consolas, monospace",
      fontSize: 13,
      theme: { background: "#0a0a12" },
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
      <div style={{ padding: 6, fontSize: 12, background: "#0d0d18" }}>
        <strong>{session.label ?? session.id.slice(0, 8)}</strong>
        {" · "}
        {session.command} {session.args.join(" ")}
        {" · "}
        <span className={`term-status term-status-${session.status}`}>{session.status}</span>
        {replayMode && <span style={{ marginLeft: 8, color: "#b48ef0" }}>(replay)</span>}
      </div>
      <div
        ref={containerRef}
        style={{ flex: 1, minHeight: 0, background: "#0a0a12" }}
      />
    </div>
  );
}
