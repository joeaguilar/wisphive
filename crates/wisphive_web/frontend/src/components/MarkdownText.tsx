import { Fragment, type ReactNode, useMemo, useState } from "react";
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

function safeHref(rawHref: string): string | null {
  const href = rawHref.trim();
  if (!href || hasUnsafeHrefChar(href)) return null;

  try {
    const parsed = new URL(href, window.location.origin);
    if (parsed.protocol === "http:" || parsed.protocol === "https:" || parsed.protocol === "mailto:") {
      return href;
    }
  } catch {
    return null;
  }

  return null;
}

function hasUnsafeHrefChar(href: string): boolean {
  for (let i = 0; i < href.length; i += 1) {
    const code = href.charCodeAt(i);
    if (code <= 0x20 || code === 0x7f) return true;
  }
  return false;
}

function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const tokenRe = /\[([^\]]+)\]\(([^)]+)\)|`([^`\n]+?)`|\*\*([^*\n]+?)\*\*/g;
  let last = 0;
  let tokenIndex = 0;

  for (const match of text.matchAll(tokenRe)) {
    const start = match.index ?? 0;
    if (start > last) nodes.push(text.slice(last, start));

    const key = `${keyPrefix}-inline-${tokenIndex++}`;
    const [, linkText, linkHref, codeText, boldText] = match;
    if (linkText !== undefined && linkHref !== undefined) {
      const href = safeHref(linkHref);
      if (href) {
        nodes.push(
          <a key={key} href={href} target="_blank" rel="noreferrer noopener">
            {linkText}
          </a>,
        );
      } else {
        nodes.push(match[0]);
      }
    } else if (codeText !== undefined) {
      nodes.push(<code key={key} className="md-code">{codeText}</code>);
    } else if (boldText !== undefined) {
      nodes.push(<strong key={key}>{boldText}</strong>);
    }

    last = start + match[0].length;
  }

  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function splitTableRow(row: string): string[] {
  return row.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((c) => c.trim());
}

function isSeparatorRow(cells: string[]): boolean {
  return cells.every((c) => /^:?-{2,}:?$/.test(c));
}

function renderTextBlocks(src: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const lines = src.split("\n");

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (!line.trim()) {
      nodes.push(<br key={`${keyPrefix}-blank-${i}`} />);
      continue;
    }

    const tableLines: string[] = [];
    let j = i;
    while (j < lines.length && /^\|.*\|\s*$/.test(lines[j])) {
      tableLines.push(lines[j]);
      j += 1;
    }
    if (tableLines.length >= 2) {
      const rows = tableLines.map(splitTableRow);
      if (isSeparatorRow(rows[1])) {
        const bodyRows = rows.slice(2).filter((r) => !isSeparatorRow(r));
        nodes.push(
          <table key={`${keyPrefix}-table-${i}`} className="md-table">
            <thead>
              <tr>
                {rows[0].map((cell, idx) => (
                  <th key={idx}>{renderInline(cell, `${keyPrefix}-table-${i}-h-${idx}`)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {bodyRows.map((row, rowIdx) => (
                <tr key={rowIdx}>
                  {row.map((cell, cellIdx) => (
                    <td key={cellIdx}>{renderInline(cell, `${keyPrefix}-table-${i}-${rowIdx}-${cellIdx}`)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>,
        );
        i = j - 1;
        continue;
      }
    }

    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading) {
      const [, marks, body] = heading;
      const content = renderInline(body, `${keyPrefix}-heading-${i}`);
      if (marks.length === 1) {
        nodes.push(<h2 key={`${keyPrefix}-h1-${i}`} className="md-h1">{content}</h2>);
      } else if (marks.length === 2) {
        nodes.push(<h3 key={`${keyPrefix}-h2-${i}`} className="md-h2">{content}</h3>);
      } else {
        nodes.push(<h4 key={`${keyPrefix}-h3-${i}`} className="md-h3">{content}</h4>);
      }
      continue;
    }

    const bullet = /^\s*[-*]\s+(.+)$/.exec(line);
    if (bullet) {
      nodes.push(
        <div key={`${keyPrefix}-li-${i}`} className="md-li">
          {"• "}
          {renderInline(bullet[1], `${keyPrefix}-li-${i}`)}
        </div>,
      );
      continue;
    }

    const numbered = /^\s*(\d+)\.\s+(.+)$/.exec(line);
    if (numbered) {
      nodes.push(
        <div key={`${keyPrefix}-nli-${i}`} className="md-li">
          {numbered[1]}. {renderInline(numbered[2], `${keyPrefix}-nli-${i}`)}
        </div>,
      );
      continue;
    }

    nodes.push(
      <Fragment key={`${keyPrefix}-line-${i}`}>
        {renderInline(line, `${keyPrefix}-line-${i}`)}
        {i < lines.length - 1 ? <br /> : null}
      </Fragment>,
    );
  }

  return nodes;
}

function renderMarkdown(src: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const lines = src.split("\n");
  let pendingText: string[] = [];

  const flushText = (key: string) => {
    if (pendingText.length === 0) return;
    nodes.push(...renderTextBlocks(pendingText.join("\n"), key));
    pendingText = [];
  };

  for (let i = 0; i < lines.length; i += 1) {
    if (/^```/.test(lines[i])) {
      flushText(`text-${i}`);
      const codeLines: string[] = [];
      i += 1;
      while (i < lines.length && !/^```/.test(lines[i])) {
        codeLines.push(lines[i]);
        i += 1;
      }
      nodes.push(
        <pre key={`code-${i}`} className="md-pre">
          <code>{codeLines.join("\n")}</code>
        </pre>,
      );
      continue;
    }

    pendingText.push(lines[i]);
  }

  flushText("text-tail");
  return nodes;
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
        <div className="markdown-content">{renderMarkdown(text)}</div>
      ) : (
        <pre className={className ?? "code-block"}>{text}</pre>
      )}
    </div>
  );
}
