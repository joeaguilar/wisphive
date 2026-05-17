# Daemon, state, and async behavior

## Timeout / dropped channel ⇒ approve

**Severity: Critical (policy)**

```310:321:crates/wisphive_daemon/src/server.rs
            let rich = match tokio::time::timeout(Duration::from_secs(timeout_secs), rx).await {
                Ok(Ok(rich)) => rich,
                Ok(Err(_)) => {
                    warn!(%id, "decision channel dropped, defaulting to approve");
                    RichDecision::approve()
                }
                Err(_) => {
                    warn!(%id, "hook timed out after {timeout_secs}s, defaulting to approve");
                    RichDecision::approve()
                }
            };
```

Same philosophy as the hook: dropped resolver or timeout **approves**. Operators relying on Wisphive as a hard gate should understand this.

---

## `Ask` decisions vs `pending_decisions` rows

**Severity: High (data consistency)**

Hook handler skips DB resolution when the outcome is `Ask`:

```331:334:crates/wisphive_daemon/src/server.rs
            if rich.decision != Decision::Ask {
                ctx.state_db.resolve_pending(id, rich.decision).await?;
            }
```

TUI/web `ClientMessage::Ask` resolves the in-memory queue but comments indicate Ask is not persisted to audit:

```563:567:crates/wisphive_daemon/src/server.rs
                            ClientMessage::Ask { id } => {
                                info!(?device_id, %id, "ask");
                                let mut q = ctx.queue.lock().await;
                                q.resolve(id, RichDecision::from(Decision::Ask));
                                // Ask/defer decisions are not persisted to the audit log
                            }
```

After `persist_pending` at enqueue time, a defer/ask flow can leave **`pending_decisions` stuck** while the UI queue no longer shows the item — confusing for crash recovery, history, and any code assuming DB reflects queue state.

**Suggestion:** Persist a terminal state (e.g. resolved-as-deferred) or delete pending rows with an explicit status; keep DB and queue semantics documented and tested.

---

## Corrupt `auto-approve.json`

**Severity: Medium**

```1132:1135:crates/wisphive_daemon/src/server.rs
    let mut config: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
```

Parse failure silently replaces config with `{}`, then the rest of the function may rewrite the file — operators can lose lists without a loud error.

---

## Broadcast channel back-pressure

**Severity: Medium (UX / observability)**

`tui_tx` uses a bounded broadcast; slow consumers drop events (`send` errors ignored). Acceptable for responsiveness but means **no guarantee** every client sees every event — document for anyone building auditing on the stream alone.

---

## macOS notification escaping

**Severity: Low–Medium (reliability)**

`escape_applescript` only handles `\` and `"`. Unusual characters or newlines in interpolated summaries can break AppleScript or produce odd banners. Unlikely to be RCE in typical `osascript -e` usage but worth hardening for robustness.

---

## Unix socket permissions

**Severity: Low**

Socket bind relies on `~/.wisphive` directory permissions. Fine when home is `700`; weaker home permissions weaken multi-user hosts.

---

## Terminal PTY pipeline

**Severity: Low (design tradeoff)**

`terminal.rs` uses `spawn_blocking` for PTY writes and a dedicated reader thread with `blocking_send` into the DB batcher — intentional to avoid losing audit data. Poisoned mutex `expect`s will panic the worker thread if violated (standard Rust pattern but worth monitoring).

---

## Shutdown / signals (`shutdown.rs`)

**Severity: Low**

Uses `expect` when registering signal handlers and `unsafe { libc::kill }` for PID probing — panics or FFI only if assumptions fail; acceptable if documented as process-fatal setup.
