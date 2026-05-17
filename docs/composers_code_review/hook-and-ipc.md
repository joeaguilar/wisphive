# Hook, IPC, and protocol

## Fail-open hook (`wisphive_hook`)

**Severity: Critical (documented product policy, still a risk)**

```68:74:crates/wisphive_hook/src/main.rs
fn main() {
    // Any failure = exit 0 (allow). Wisphive is fail-open.
    let code = match run() {
        Ok(resp) => format_and_exit(&resp),
        Err(_) => 0,
    };
    process::exit(code);
}
```

Any error path from `run()` (I/O, JSON parse, daemon protocol mismatch, etc.) exits **0**, which approves from Claude Code’s perspective. This matches project docs but means availability or correctness bugs translate into **silent approval**. Operators should treat daemon/hook health as part of their security posture.

---

## Unexpected server message ⇒ approve

**Severity: High (policy / correctness)**

```477:494:crates/wisphive_hook/src/main.rs
    match response {
        ServerMessage::DecisionResponse {
            decision,
            message,
            updated_input,
            additional_context,
            selected_permission,
            ..
        } => Ok(HookResponse {
            decision,
            message,
            updated_input,
            additional_context,
            selected_permission,
            event_type,
        }),
        _ => Ok(HookResponse::simple(Decision::Approve)),
    }
```

If the daemon ever sends a non-`DecisionResponse` line after the handshake (bug, protocol skew, middle component), the hook returns **Approve**. Combined with global fail-open, this is a second layer of “default allow.”

**Suggestion:** Map unknown variants to **deny** or **ask**, or exit with a distinct code documented for operators — only if product policy allows tightening fail-open.

---

## Unbounded stdin read

**Severity: Medium (DoS on hook process)**

The hook reads full stdin into a string for parsing. A buggy or hostile caller can supply arbitrarily large JSON and pressure memory. Bound reads or a max length with explicit deny/error handling would harden the subprocess.

---

## Blocking read after clearing socket timeout

**Severity: Medium (availability)**

After sending `DecisionRequest`, the hook clears read timeout then blocks on `read_line` for the decision. The daemon applies its own timeout, but if the peer stalls without closing cleanly, behavior depends on OS/socket semantics. Worth aligning with an explicit maximum wait on the hook side for defense in depth.

---

## PermissionRequest serialization fallback

**Severity: Medium**

Uses `serde_json::to_value(...).unwrap_or(json!([]))` for permission payloads — serialization failure collapses to an empty list, which may not match intended PermissionRequest semantics.

---

## Auto-approve audit JSONL

**Severity: Low–Medium**

Audit append paths use `unwrap_or_default()` on JSON serialization and may ignore write errors; corruption or silent loss of audit lines is possible under disk pressure.

---

## Protocol decoding (`wisphive_protocol`)

**Severity: Medium (resource limits)**

`decode` is effectively `serde_json::from_str` on newline-framed input without a documented global max line length or depth cap. Trusted local sockets reduce exposure, but malicious or buggy peers could still cause large allocations or CPU during parse.

**Suggestion:** Document trusted-boundary assumptions or add conservative caps at read sites.

---

## Arbitrary JSON in `DecisionRequest`

**Severity: Low (design)**

`tool_input` and related fields use `serde_json::Value`, so nesting/size are bounded only by serde defaults and caller behavior. Fine for local IPC if documented; tighten if the socket boundary ever widens.
