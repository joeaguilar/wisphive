# Web server and frontend

## `MarkdownText`: link href injection (XSS-style)

**Severity: Critical (stored/reflected HTML in authenticated UI)**

```62:72:crates/wisphive_web/frontend/src/components/MarkdownText.tsx
  text = text
    .replace(/^### (.+)$/gm, '<h4 class="md-h3">$1</h4>')
    ...
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>')
```

The body of the markdown is HTML-escaped **before** structural replacements, but **link targets `$2` are not validated or escaped**. A string like `[click](javascript:alert(1))` becomes an executable URL in the SPA DOM. Content coming from tool payloads or agent text could abuse this in-browser.

**Mitigation ideas:** Allowlist schemes (`http:`, `https:`, `mailto:`), reject `javascript:`, HTML-escape href, or render markdown via a hardened library with safe link handling.

---

## `dangerouslySetInnerHTML`

**Severity: Medium (by design, elevated by custom renderer)**

```120:124:crates/wisphive_web/frontend/src/components/MarkdownText.tsx
      {view === "rendered" ? (
        <div
          className="markdown-content"
          dangerouslySetInnerHTML={{ __html: renderMarkdown(text) }}
```

Acceptable only if `renderMarkdown` is proven safe for every input path. The custom regex pipeline is easy to get wrong (see links above; also verify no double-unescape paths).

---

## Web ↔ daemon IPC (`reauth_ipc`)

**Severity: Medium (trust boundary)**

Any local process that can connect to the daemon’s Unix socket can send messages the daemon treats as authoritative for some actions (e.g. marking a device fresh). This matches single-user threat models; document that **socket permissions are the gate**.

---

## RNG failure in device token generation (`auth.rs`)

**Severity: Medium (availability)**

Uses `expect` on OS RNG failure — fails hard rather than returning a structured error. Rare but abrupt.

---

## Layered CSRF / Origin handling

**Severity: Info**

`security.rs` documents absent `Origin` for navigational requests; pairing with bearer/device tokens is the intended model. No issue found in static pass — keep regression tests if Origin logic changes.

---

## WS bridge typing

**Severity: Positive note**

`ws_bridge` decodes structured commands rather than forwarding opaque bytes for sensitive fields — reduces spoofing risk for things like `device_id`.

---

## ESLint suppressions

**Severity: Low (maintainability)**

Targeted `eslint-disable-next-line` comments appear in:

- `Config.tsx` — `react-hooks/set-state-in-effect`
- `useAuth.ts` — same rule
- `Terminals.tsx`, `TerminalView.tsx` — `react-hooks/exhaustive-deps`
- `useWisphive.ts` — `react-hooks/immutability`

Each should carry a one-line rationale or be refactored so hooks stay lint-clean long term.
