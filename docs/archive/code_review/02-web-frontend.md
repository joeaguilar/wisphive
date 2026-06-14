# Web and Frontend Findings

## High: Custom markdown renderer can inject attacker-controlled HTML attributes

Affected code:

- `crates/wisphive_web/frontend/src/components/MarkdownText.tsx`

Evidence:

- `renderMarkdown` escapes `&`, `<`, and `>`.
- It later converts markdown links with:
  - `'<a href="$2" target="_blank" rel="noreferrer">$1</a>'`
- The URL capture is inserted directly into an HTML attribute.
- Quotes are not escaped, and URL schemes are not restricted.
- The resulting HTML is injected with `dangerouslySetInnerHTML`.

Impact:

- Any markdown text sourced from tool output, history, prompts, or model output can inject attributes such as `onmouseover` or `style`, depending on browser parsing.
- A successful XSS can steal the device bearer token from `localStorage`, then drive `/ws` and `/api/*` as that trusted device.

Recommended fix:

- Replace the custom renderer with a maintained markdown pipeline that sanitizes HTML, such as `react-markdown` plus `rehype-sanitize`, or render markdown to React nodes without `dangerouslySetInnerHTML`.
- If a custom renderer remains, escape attribute values, block `javascript:`/`data:` URLs, and avoid raw HTML injection.

Test suggestion:

- Add component tests for malicious markdown like `[x](" onmouseover="alert(1))` and `[x](javascript:alert(1))`; assert no executable attributes or dangerous URLs appear.

## High: Terminal output side effects run inside a React state updater

Affected code:

- `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`

Evidence:

- `handleMessage` calls `setState((prev) => { switch (...) { ... } })`.
- The `term_chunk`, `term_catchup`, and `term_replay_chunk` cases call terminal output handlers inside that state updater.
- The same updater also mutates `document.title` and sends browser notifications in some branches.

Impact:

- React requires state updater functions to be pure.
- In React Strict Mode and future concurrent rendering paths, React can invoke updater functions more than once.
- Duplicating a terminal output side effect can corrupt the xterm buffer, replay chunks twice, or show duplicate notifications.

Recommended fix:

- Parse the incoming message once, route terminal/browser side effects outside `setState`, and use `setState` only for state transitions.
- A clean pattern is:
  - Handle terminal chunks and notifications before/after state updates.
  - Use `setState` only for queue/agents/history arrays.

Test suggestion:

- Wrap the hook/component in Strict Mode and feed a `term_chunk`; assert the registered terminal handler is invoked exactly once.

## High: Multiple sudo-gated approvals can strand older requests after reauth

Affected code:

- `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`

Evidence:

- `approveStashRef` stores approvals by request id.
- When `web_reauth_required` arrives, `pendingReauth` is replaced with the newest request id.
- The comment says both old and new stashes will be replayed, with the older one via a secondary drain.
- `retryPendingApprove` only reads and sends the single current `pendingReauth.request_id`; no secondary drain exists.

Impact:

- If a user approves two sudo-class requests before reauthing, the second gate replaces `pendingReauth`.
- Reauth only retries the newest approve.
- Older requests remain queued with a stale stash and no modal prompting the user to retry them.

Recommended fix:

- Model `pendingReauth` as a queue/set of gated request ids, or drain all stashed approvals that still correspond to queued requests after one successful reauth.
- Keep the modal text focused on the current request, but replay all eligible gated approvals once the daemon marks the device fresh.

Test suggestion:

- Simulate two `web_reauth_required` messages, then call `retryPendingApprove`; assert both queued request ids are approved or that the UI presents the remaining request.

## Medium: Frontend lint currently fails

Affected code:

- `crates/wisphive_web/frontend/src/components/MarkdownText.tsx`
- `crates/wisphive_web/frontend/src/components/Queue.tsx`
- `crates/wisphive_web/frontend/src/components/Sessions.tsx`
- `crates/wisphive_web/frontend/src/components/TerminalQueueDock.tsx`

Evidence:

- `npm run lint` reports 7 errors:
  - `MarkdownText.tsx`: `no-control-regex`
  - `Queue.tsx`: four `react-refresh/only-export-components` errors
  - `Sessions.tsx`: `react-hooks/set-state-in-effect`
  - `TerminalQueueDock.tsx`: `react-hooks/set-state-in-effect`

Impact:

- The CI frontend lint job fails.
- The build passing can mask these until CI reaches the lint step.

Recommended fix:

- Replace the `\u0000` regex placeholder in the markdown renderer or suppress with a narrower local justification if kept.
- Move exported non-component helpers from `Queue.tsx` into a utility module.
- Refactor derived-state reset patterns in `Sessions.tsx` and `TerminalQueueDock.tsx` so state is derived from props or reset through event handlers rather than synchronous effects.

Test suggestion:

- Keep `npm run lint` in CI and consider running lint before build to fail faster.

## Medium: Project live-status data is emitted but not represented or used correctly

Affected code:

- `crates/wisphive_protocol/src/types.rs`
- `crates/wisphive_daemon/src/server.rs`
- `crates/wisphive_web/frontend/src/types/protocol.ts`
- `crates/wisphive_web/frontend/src/components/Projects.tsx`

Evidence:

- Rust `ProjectSummary` includes `pending_count` and `has_live_agents`.
- The daemon populates both fields before sending `projects_response`.
- The frontend `ProjectSummary` type omits both fields.
- `Projects.tsx` uses `p.agent_count > 0` to render the live/ended indicator.

Impact:

- Any historical project with at least one past agent appears "live" even when no agent is currently connected.
- Pending counts sent by the daemon cannot be shown or type-checked in the frontend.

Recommended fix:

- Add `pending_count` and `has_live_agents` to the frontend protocol type.
- Change the status indicator to use `has_live_agents`.
- Show `pending_count` where useful, matching the sessions UI.

Test suggestion:

- Add a UI test or type-level fixture where `agent_count = 3` and `has_live_agents = false`; assert the project renders as ended.

## Medium: Terminal replay ignores input and resize events in the frontend

Affected code:

- `crates/wisphive_web/frontend/src/hooks/useWisphive.ts`
- `crates/wisphive_daemon/src/terminal.rs`
- `crates/wisphive_protocol/src/types.rs`

Evidence:

- The protocol records `TerminalDirection::Input`, `Output`, and `Resize`.
- `term_replay_chunk` handling invokes the terminal handler only when `msg.direction === "output"`.
- `term_chunk` similarly ignores live input and resize messages.

Impact:

- Replay is not faithful for sessions where input is not echoed by the shell or where resize events are needed to reproduce screen state.
- Recorded terminal data is richer than what the frontend uses.

Recommended fix:

- Decide whether replay is output-only or full-fidelity.
- If full-fidelity, teach the frontend to apply resize events and optionally render input events in a controlled way.
- If output-only, update protocol/docs/UI labels so users do not expect exact replay.

Test suggestion:

- Record output, input, and resize events; replay them through the frontend handler and assert expected xterm resize/write calls.

## Low: No frontend unit/component tests cover protocol and rendering behavior

Affected code:

- `crates/wisphive_web/frontend`

Evidence:

- `package.json` has `dev`, `build`, `lint`, and `preview` scripts, but no test script or test dependencies.
- High-risk code includes custom markdown rendering, WebSocket protocol handling, terminal streaming, and auth-token state.

Impact:

- Protocol regressions and XSS fixes rely on manual testing.
- React hook behavior around reconnects, reauth, and terminal chunks is hard to validate through Rust tests.

Recommended fix:

- Add a lightweight frontend test stack such as Vitest plus React Testing Library.
- Start with tests for `MarkdownText`, `useWisphive` message handling, auth token clearing, and sudo reauth retry behavior.

Test suggestion:

- Add `npm test` and include it in CI after lint/build.

## Low: Production bundle exceeds Vite's default chunk warning threshold

Affected code:

- `crates/wisphive_web/frontend`

Evidence:

- `npm run build` reports `dist/assets/index-*.js` at about 602 kB minified, above Vite's 500 kB chunk warning threshold.

Impact:

- Initial load on phone/LAN clients is heavier than necessary.
- This is not a correctness bug today, but terminal/xterm code is a likely candidate for lazy loading.

Recommended fix:

- Consider code splitting heavy terminal views and xterm-related dependencies behind the terminal route/panel.

Test suggestion:

- Track bundle size in CI after code splitting to prevent regressions.
