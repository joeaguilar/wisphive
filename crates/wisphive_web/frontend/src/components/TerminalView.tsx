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

    // Touch-to-scroll (itr#445). xterm's viewport scrolls via the wheel or the
    // scrollbar, but a phone/tablet has neither — and xterm 6's own touch
    // Gesture does not scroll this build — so a vertical touch-drag has to be
    // translated into a scrollback scroll by hand. We drive the public
    // `term.scrollToLine()` API with an ABSOLUTE target computed from the
    // gesture's anchor (the viewport-top buffer line captured at touchstart)
    // plus the finger's pixel travel in rows. Absolute positioning — rather than
    // an accumulated per-move delta — means a boundary clamp at the top of
    // history or the live tail cannot desync the gesture: xterm clamps
    // scrollToLine internally and a reversal still tracks the finger (itr#477;
    // the old accumulator advanced by *requested* rows and drifted at clamps).
    // Listeners are native + capture phase with { passive: false } because
    // React's synthetic touch handlers are passive and cannot preventDefault;
    // stopPropagation keeps xterm from starting a text selection mid-scroll. A
    // tap (no movement past the threshold) is never intercepted, so tap-to-focus
    // + the on-screen keyboard keep working. `touch-action: pinch-zoom` on the
    // container (set inline below) keeps an ancestor overflow:auto pane from
    // claiming the single-finger pan while still allowing pinch-zoom (itr#478).
    const container = containerRef.current;
    const viewport = container.querySelector<HTMLElement>(".xterm-viewport");
    const SCROLL_LOCK_PX = 6; // movement before a drag commits to scrolling
    let activeTouchId: number | null = null;
    let startY = 0;
    let anchorTop = 0; // viewport-top buffer line captured at gesture start
    let scrolling = false;
    let appliedOffset = 0; // rows already applied this gesture (vs anchor)

    // Height of one terminal row in CSS px. The viewport is exactly rows tall,
    // so its clientHeight / rows is the cell height; fall back to a font-size
    // estimate if layout hasn't settled (clientHeight can be 0 pre-paint).
    const rowHeight = () => {
      const h = viewport?.clientHeight ?? 0;
      const rows = term.rows || 1;
      const px = h > 0 ? h / rows : 0;
      return px > 0 ? px : 17; // ~fontSize(13) * lineHeight
    };

    const onTouchStart = (e: TouchEvent) => {
      if (e.touches.length !== 1) return;
      const t = e.touches[0];
      activeTouchId = t.identifier;
      startY = t.clientY;
      anchorTop = term.buffer.active.viewportY;
      scrolling = false;
      appliedOffset = 0;
    };
    const onTouchMove = (e: TouchEvent) => {
      if (activeTouchId === null) return;
      const t = Array.from(e.touches).find((x) => x.identifier === activeTouchId);
      if (!t) return;
      const dy = t.clientY - startY;
      if (!scrolling && Math.abs(dy) < SCROLL_LOCK_PX) return;
      // Drag down (dy>0) → reveal earlier scrollback (a smaller line index).
      const offsetRows = Math.round(-dy / rowHeight());
      // Only intercept once the gesture actually moves the viewport by a row.
      // A 6-8px finger jitter rounds to zero rows with a ~17px row height, so
      // preventDefault-ing there would swallow a tap without scrolling (itr#480).
      // Once a non-zero delta has been applied we keep tracking the finger —
      // including back through the anchor — so a reversal still follows (itr#477).
      if (offsetRows === appliedOffset) return;
      scrolling = true;
      appliedOffset = offsetRows;
      // Command an absolute target from the anchor; xterm clamps at the bounds.
      term.scrollToLine(anchorTop + offsetRows);
      e.preventDefault();
      e.stopPropagation();
    };
    const onTouchEnd = (e: TouchEvent) => {
      if (activeTouchId === null) return;
      if (Array.from(e.touches).some((x) => x.identifier === activeTouchId)) return;
      activeTouchId = null;
      scrolling = false;
      appliedOffset = 0;
    };
    container.addEventListener("touchstart", onTouchStart, { capture: true, passive: true });
    container.addEventListener("touchmove", onTouchMove, { capture: true, passive: false });
    container.addEventListener("touchend", onTouchEnd, { capture: true, passive: true });
    container.addEventListener("touchcancel", onTouchEnd, { capture: true, passive: true });

    return () => {
      cancelAnimationFrame(rafId);
      resizeObserver.disconnect();
      container.removeEventListener("touchstart", onTouchStart, { capture: true });
      container.removeEventListener("touchmove", onTouchMove, { capture: true });
      container.removeEventListener("touchend", onTouchEnd, { capture: true });
      container.removeEventListener("touchcancel", onTouchEnd, { capture: true });
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
        // touchAction: pinch-zoom — the handler (itr#445) owns single-finger
        // vertical drags (so no ancestor overflow pane pans), while two-finger
        // pinch-zoom of the terminal stays available for low-vision users (itr#478).
        style={{ flex: 1, minHeight: 0, background: "var(--bg)", touchAction: "pinch-zoom" }}
      />
    </div>
  );
}
