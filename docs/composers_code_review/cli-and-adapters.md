# CLI, hooks installer, adapters

## `daemon start`: invalid host skips **entire** daemon start when web is implied

**Severity: Critical (silent failure)**

```446:477:crates/wisphive_cli/src/main.rs
                        let web_requested = web || web_dev || host != "127.0.0.1" || port != 3100;
                        let web_opts = if web_requested {
                            ...
                            let Some(host_octets) = parse_host_octets(&host) else {
                                return Ok(());
                            };
                            ...
                            Some(commands::daemon::WebOptions { ... })
                        } else {
                            None
                        };
                        commands::daemon::start(web_opts).await
```

If `web_requested` is true (e.g. `--port 8080` or `--web`) but `parse_host_octets` returns `None`, the closure returns **`Ok(())` immediately** and **`commands::daemon::start` is never called**. The daemon does not start; exit code is still success. Stderr may show “Invalid host address” from `parse_host_octets`, but the combination is easy to miss in scripts.

**Suggestion:** Return a non-zero exit code and/or still start the daemon without web when host parse fails; never treat “daemon start” as successful when start was skipped.

---

## Same pattern in `serve_web`

**Severity: High**

```575:577:crates/wisphive_cli/src/main.rs
    let Some(host_octets) = parse_host_octets(&host) else {
        return Ok(());
    };
```

Invalid host ⇒ exit 0 with no server — problematic for automation.

---

## Web enabled by non-default host/port

**Severity: Medium (footgun)**

`web_requested = web || web_dev || host != "127.0.0.1" || port != 3100` means changing host or port alone turns on embedded web serving. Document prominently; some users may only intend to move the **daemon** listening behavior.

---

## `hooks install`: panic on malformed `hooks` key

**Severity: Medium**

```241:244:crates/wisphive_cli/src/commands/hooks.rs
pub(crate) fn add_hook_entry(settings: &mut serde_json::Value, hook_type: &str, command: &str) {
    let hooks = settings["hooks"]
        .as_object_mut()
        .expect("hooks should be an object");
```

If `.claude/settings.json` has `"hooks": []` or a string, **`wisphive hooks install` panics** instead of a friendly error.

**Suggestion:** Return `Result` with context (“hooks must be an object”).

---

## User config parse failures (`wisphive_daemon` / `config.rs`)

**Severity: Medium**

Reported behavior: invalid `config.json` falls back to defaults silently — operators lose throttle/timeouts without noticing.

---

## Spawned agents: `--dangerously-skip-permissions`

**Severity: Info (intentional, document)**

```37:40:crates/wisphive_daemon/src/process_registry.rs
        // Non-interactive print mode
        cmd.arg("-p");

        // Wisphive is the gatekeeper — skip Claude's own permission prompts
        cmd.arg("--dangerously-skip-permissions");
```

Correct given Wisphive’s model; ensure docs warn that **managed agents trust Wisphive** as the only gate.

---

## `wisphive_adapters`

**Severity: Info**

Non-Claude adapters are stubs — fine if documented; avoid implying parity with the hook-based path until implemented.
