import { useMemo, useState } from "react";
import { CopyButton } from "./CopyButton";

// Heuristic: detect markdown-ish formatting worth offering a rendered view for.
function looksLikeMarkdown(text: string): boolean {
  if (!text) return false;
  const patterns = [
    /^#{1,6}\s+\S/m,        // headers
    /^\s*[-*]\s+\S/m,       // bullet lists
    /^\s*\d+\.\s+\S/m,      // numbered lists
    /\*\*[^*\n]+\*\*/,      // bold
    /`[^`\n]+`/,            // inline code
    /^```/m,                // code fences
    /^\s*\|.+\|\s*$/m,      // table rows
    /\[[^\]]+\]\([^)]+\)/,  // links
  ];
  return patterns.some((re) => re.test(text));
}

// Simple markdown → HTML. Handles: fenced code, headers, bold, inline code,
// lists, tables (pipe syntax), links, and paragraph breaks.
function renderMarkdown(src: string): string {
  const escape = (s: string) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  // Extract fenced code blocks first so their contents aren't mangled.
  const codeBlocks: string[] = [];
  let text = src.replace(/```(\w*)\n([\s\S]*?)```/g, (_m, _lang, body) => {
    const idx = codeBlocks.length;
    codeBlocks.push(`<pre class="md-pre"><code>${escape(body)}</code></pre>`);
    return `\u0000CODE${idx}\u0000`;
  });

  text = escape(text);

  // Tables: consecutive lines starting & ending with |, with a separator row.
  text = text.replace(
    /((?:^\|.*\|\s*\n)+)/gm,
    (block) => {
      const rows = block.trim().split("\n").map((r) =>
        r.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim())
      );
      if (rows.length < 2) return block;
      const isSep = (cells: string[]) => cells.every((c) => /^:?-{2,}:?$/.test(c));
      let header: string[] | null = null;
      let bodyStart = 0;
      if (rows.length >= 2 && isSep(rows[1])) {
        header = rows[0];
        bodyStart = 2;
      }
      const body = rows.slice(bodyStart).filter((r) => !isSep(r));
      const thead = header
        ? `<thead><tr>${header.map((c) => `<th>${c}</th>`).join("")}</tr></thead>`
        : "";
      const tbody = `<tbody>${body
        .map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`)
        .join("")}</tbody>`;
      return `<table class="md-table">${thead}${tbody}</table>`;
    }
  );

  text = text
    .replace(/^### (.+)$/gm, '<h4 class="md-h3">$1</h4>')
    .replace(/^## (.+)$/gm, '<h3 class="md-h2">$1</h3>')
    .replace(/^# (.+)$/gm, '<h2 class="md-h1">$1</h2>')
    .replace(/\*\*([^*\n]+?)\*\*/g, "<strong>$1</strong>")
    .replace(/`([^`\n]+?)`/g, '<code class="md-code">$1</code>')
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>')
    .replace(/^\s*[-*] (.+)$/gm, '<div class="md-li">• $1</div>')
    .replace(/^\s*(\d+)\. (.+)$/gm, '<div class="md-li">$1. $2</div>')
    .replace(/\n\n+/g, "<br/><br/>")
    .replace(/\n/g, "<br/>");

  // Restore code blocks.
  text = text.replace(/\u0000CODE(\d+)\u0000/g, (_m, i) => codeBlocks[Number(i)]);
  return text;
}

interface MarkdownTextProps {
  text: string;
  className?: string;
}

export function MarkdownText({ text, className }: MarkdownTextProps) {
  const isMarkdown = useMemo(() => looksLikeMarkdown(text), [text]);
  const [view, setView] = useState<"rendered" | "raw">(isMarkdown ? "rendered" : "raw");

  if (!isMarkdown) {
    return (
      <div className="code-block-wrap">
        <CopyButton value={text} className="copy-btn-overlay" />
        <pre className={className ?? "code-block"}>{text}</pre>
      </div>
    );
  }

  return (
    <div className="md-text">
      <div className="md-tabs" role="tablist" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          role="tab"
          aria-selected={view === "rendered"}
          className={`md-tab ${view === "rendered" ? "active" : ""}`}
          onClick={(e) => { e.stopPropagation(); setView("rendered"); }}
        >
          Rendered
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={view === "raw"}
          className={`md-tab ${view === "raw" ? "active" : ""}`}
          onClick={(e) => { e.stopPropagation(); setView("raw"); }}
        >
          Raw
        </button>
        <CopyButton value={text} className="copy-btn-tab" title="Copy markdown source" />
      </div>
      {view === "rendered" ? (
        <div
          className="markdown-content"
          dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }}
        />
      ) : (
        <pre className={className ?? "code-block"}>{text}</pre>
      )}
    </div>
  );
}
