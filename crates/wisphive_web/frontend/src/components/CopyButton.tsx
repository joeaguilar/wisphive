import { useCallback, useEffect, useRef, useState } from "react";
import type React from "react";

interface CopyButtonProps {
  /** Static text or a function returning text (lazy — useful when the source is large or computed). */
  value: string | (() => string);
  label?: string;
  copiedLabel?: string;
  className?: string;
  title?: string;
}

export function CopyButton({
  value,
  label = "Copy",
  copiedLabel = "Copied!",
  className,
  title,
}: CopyButtonProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
  }, []);

  const onClick = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    const text = typeof value === "function" ? value() : value;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
      } else {
        const ta = document.createElement("textarea");
        ta.value = text;
        ta.style.position = "fixed";
        ta.style.opacity = "0";
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        document.body.removeChild(ta);
      }
      setCopied(true);
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 1200);
    } catch {
      setCopied(false);
    }
  }, [value]);

  return (
    <button
      type="button"
      className={`copy-btn ${className ?? ""}`.trim()}
      onClick={onClick}
      title={title ?? label}
      aria-label={title ?? label}
    >
      {copied ? copiedLabel : label}
    </button>
  );
}
