mod commands;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "wisphive",
    version,
    about = "Agent control plane for multiplexed AI workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Launch the TUI client
    Tui,

    /// Web UI server + admin (password, devices, TLS fingerprint)
    Web {
        #[command(subcommand)]
        action: WebAction,
    },

    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Hook management for Claude Code integration
    Hooks {
        #[command(subcommand)]
        action: HookAction,
    },

    /// Emergency kill switch — disables all hooks instantly
    EmergencyOff,

    /// Manage AI agent processes
    Agent {
        #[command(subcommand)]
        action: Box<AgentAction>,
    },

    /// Browse and search the audit history
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },

    /// View or change daemon configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Check setup and diagnose issues
    Doctor {
        /// Project directory to check (defaults to current directory)
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },

    /// Manage wisphive-owned terminal (PTY) sessions
    Term {
        #[command(subcommand)]
        action: TermAction,
    },
}

/// CLI selector for the daemon's [`wisphive_web::AuthProfile`] (itr#310).
/// `local-lan` is the default; `enterprise` requires `--auth-rp-id` plus
/// (once itr#270 lands) `--tls-cert` / `--tls-key`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum AuthProfileArg {
    /// Default; self-signed TLS OK, ephemeral LAN pairing listener
    /// enabled, phone authenticates via device bearer.
    LocalLan,
    /// Operator-provided cert + real registrable domain; ephemeral LAN
    /// listener disabled, passkey-register sudo-gated, UV required.
    Enterprise,
}

#[derive(Subcommand)]
enum WebAction {
    /// Serve the Web UI (default — flags match the pre-subcommand form)
    Serve {
        /// HTTP port (default: 3100)
        #[arg(short, long, default_value = "3100")]
        port: u16,
        /// Bind address (default: 127.0.0.1, use 0.0.0.0 for LAN access)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Dev mode: only serve WebSocket, expect Vite dev server for frontend
        #[arg(long)]
        dev: bool,
        /// Suppress the first-run browser auto-open (useful for headless
        /// servers / CI / when launched from a UI wrapper that opens its own
        /// WebView).
        #[arg(long)]
        no_open: bool,
        /// Auth/security posture (itr#310). `local-lan` (default) for
        /// single-user local deploys; `enterprise` requires
        /// `--auth-rp-id` plus a user-provided TLS cert.
        #[arg(long, value_enum, default_value_t = AuthProfileArg::LocalLan)]
        auth_profile: AuthProfileArg,
        /// WebAuthn RP ID (registrable domain) for the Enterprise
        /// profile. Required when `--auth-profile enterprise`; ignored
        /// otherwise. Must NOT be an IP literal — WebAuthn forbids them.
        #[arg(long)]
        auth_rp_id: Option<String>,
    },
    /// Set the web admin password (prompts twice, stores Argon2id hash)
    SetPassword,
    /// Wipe password + every trusted device + every enrolled passkey
    ResetPassword,
    /// Manage trusted devices
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },
    /// Print the on-disk TLS certificate's SHA-256 fingerprint
    Fingerprint,
}

#[derive(Subcommand)]
enum DevicesAction {
    /// List every device (active and revoked), newest first
    List,
    /// Revoke a device by UUID (idempotent)
    Revoke {
        /// Device UUID (from `devices list`)
        id: String,
    },
}

#[derive(Subcommand)]
enum TermAction {
    /// Spawn a new terminal session (defaults to $SHELL -l in current cwd)
    New {
        /// Human-readable label for the session
        #[arg(long)]
        label: Option<String>,
        /// Working directory for the spawned command
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        /// Command to run (defaults to $SHELL -l)
        #[arg(long)]
        cmd: Option<String>,
        /// Args passed to --cmd (repeatable)
        #[arg(long)]
        arg: Vec<String>,
        /// After creating, enter the session from this terminal
        #[arg(long)]
        attach: bool,
    },
    /// List all terminal sessions (running + historical)
    List,
    /// Attach to a running terminal session
    Attach {
        /// Session UUID
        id: String,
    },
    /// Replay a terminal session's recorded events
    Replay {
        /// Session UUID
        id: String,
        /// Playback speed multiplier (1.0 = realtime)
        #[arg(long, default_value = "1.0")]
        speed: f32,
    },
    /// Close (kill) a running terminal session
    Close {
        /// Session UUID
        id: String,
        /// Force kill the child process
        #[arg(long)]
        kill: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show all config values
    List,
    /// Get a config value
    Get {
        /// Config key (e.g. "notifications")
        key: String,
    },
    /// Set a config value
    Set {
        /// Config key (e.g. "notifications")
        key: String,
        /// Value to set (e.g. "false")
        value: String,
    },
    /// Manage the auto-approve tool list
    AutoApprove {
        #[command(subcommand)]
        action: AutoApproveAction,
    },
}

#[derive(Subcommand)]
enum AutoApproveAction {
    /// Show current level, included tools, and overrides
    Status,
    /// Set the auto-approve permission level (off, read, write, execute, all)
    Level {
        /// Permission level
        level: String,
    },
    /// Add a tool to auto-approve (override on top of level)
    Add {
        /// Tool name (e.g. "Bash")
        tool: String,
    },
    /// Remove a tool from auto-approve (queue it despite level)
    Remove {
        /// Tool name (e.g. "WebFetch")
        tool: String,
    },
    /// Reset to defaults (level: read, no overrides)
    Reset,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// Start the background daemon. Optionally also serve the web UI in the
    /// same process with `--web`.
    Start {
        /// Also serve the web UI in this process.
        #[arg(long)]
        web: bool,
        /// Web UI bind address (implies --web). Use 0.0.0.0 for LAN access.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Web UI HTTP port (implies --web).
        #[arg(long, default_value = "3100")]
        port: u16,
        /// Dev mode: only serve the WebSocket, expect Vite dev server for the frontend.
        #[arg(long)]
        web_dev: bool,
        /// Suppress the first-run browser auto-open (ignored when --web is
        /// not active). Useful for headless servers, CI, and UI wrappers
        /// that open their own WebView.
        #[arg(long)]
        no_open: bool,
        /// Auth/security posture for the embedded web UI (itr#310).
        /// `local-lan` (default) for single-user local deploys;
        /// `enterprise` requires `--auth-rp-id` plus a user-provided TLS
        /// cert.
        #[arg(long, value_enum, default_value_t = AuthProfileArg::LocalLan)]
        auth_profile: AuthProfileArg,
        /// WebAuthn RP ID (registrable domain) for the Enterprise
        /// profile. Required when `--auth-profile enterprise`; ignored
        /// otherwise.
        #[arg(long)]
        auth_rp_id: Option<String>,
    },
    /// Stop the running daemon
    Stop,
    /// Show daemon status
    Status,
}

#[derive(Subcommand)]
enum AgentAction {
    /// Start an AI agent in a project directory
    Start(Box<StartArgs>),
    /// List running agent processes
    List,
    /// Stop a running agent process
    Stop {
        /// Agent ID to stop
        agent_id: String,
    },
}

#[derive(clap::Args)]
struct StartArgs {
    /// Path to the project directory (defaults to current directory)
    #[arg(long)]
    project: Option<std::path::PathBuf>,
    /// Model to use (e.g. "sonnet", "opus")
    #[arg(long)]
    model: Option<String>,
    /// Prompt to pass to the agent
    #[arg(long)]
    prompt: String,
    /// Display name for the agent session
    #[arg(long)]
    name: Option<String>,
    /// Reasoning effort level (low, medium, high)
    #[arg(long)]
    reasoning: Option<String>,
    /// Maximum number of agentic turns
    #[arg(long)]
    max_turns: Option<u32>,
    /// Permission mode (default, plan, bypassPermissions)
    #[arg(long)]
    permission_mode: Option<String>,
    /// Custom system prompt (replaces default)
    #[arg(long)]
    system_prompt: Option<String>,
    /// Additional system prompt (appended to default)
    #[arg(long)]
    append_system_prompt: Option<String>,
    /// Restrict to specific tools (repeatable)
    #[arg(long = "allowed-tools")]
    allowed_tools: Option<Vec<String>>,
    /// Block specific tools (repeatable)
    #[arg(long = "disallowed-tools")]
    disallowed_tools: Option<Vec<String>>,
    /// Continue the most recent session
    #[arg(long = "continue", conflicts_with = "resume")]
    continue_session: bool,
    /// Resume a specific session by ID
    #[arg(long, conflicts_with = "continue_session")]
    resume: Option<String>,
    /// Output format (json, stream-json, text)
    #[arg(long)]
    output_format: Option<String>,
    /// Enable verbose output
    #[arg(long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum HistoryAction {
    /// Search history for file paths, tool names, or other text
    Search {
        /// Search query (matches file paths, commands, tool names)
        query: String,
        /// Filter by agent ID
        #[arg(long)]
        agent_id: Option<String>,
        /// Filter by tool name
        #[arg(long)]
        tool: Option<String>,
        /// Maximum results
        #[arg(long, default_value = "50")]
        limit: u32,
    },
    /// Show recent history entries
    Recent {
        /// Maximum results
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Filter by agent ID
        #[arg(long)]
        agent_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Install Wisphive hooks into a project's .claude/settings.json
    Install {
        /// Path to the project directory
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        /// Install hooks in all known projects
        #[arg(long)]
        all: bool,
    },
    /// Remove Wisphive hooks from a project's .claude/settings.json
    Uninstall {
        /// Path to the project directory
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        /// Remove hooks from all known projects
        #[arg(long)]
        all: bool,
    },
    /// Enable hooks (set mode to active)
    Enable,
    /// Disable hooks (set mode to off — instant pass-through)
    Disable,
    /// Show hook installation and mode status
    Status,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        // Daemon-independent commands (no tokio runtime needed)
        Command::Config { action } => match action {
            ConfigAction::List => commands::config::list(),
            ConfigAction::Get { key } => commands::config::get(&key),
            ConfigAction::Set { key, value } => commands::config::set(&key, &value),
            ConfigAction::AutoApprove { action } => match action {
                AutoApproveAction::Status => commands::config::auto_approve_status(),
                AutoApproveAction::Level { level } => commands::config::auto_approve_level(&level),
                AutoApproveAction::Add { tool } => commands::config::auto_approve_add(&tool),
                AutoApproveAction::Remove { tool } => commands::config::auto_approve_remove(&tool),
                AutoApproveAction::Reset => commands::config::auto_approve_reset(),
            },
        },
        Command::Doctor { project } => commands::doctor::run(project),
        Command::EmergencyOff => commands::hooks::emergency_off(),
        Command::Hooks { action } => match action {
            HookAction::Enable => commands::hooks::set_mode("active"),
            HookAction::Disable => commands::hooks::set_mode("off"),
            HookAction::Install { project, all } => commands::hooks::install(project, all),
            HookAction::Uninstall { project, all } => commands::hooks::uninstall(project, all),
            HookAction::Status => commands::hooks::status(),
        },

        // History commands (need tokio runtime for socket communication)
        Command::History { action } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                match action {
                    HistoryAction::Search {
                        query,
                        agent_id,
                        tool,
                        limit,
                    } => commands::history::search(query, agent_id, tool, limit).await,
                    HistoryAction::Recent { limit, agent_id } => {
                        commands::history::recent(limit, agent_id).await
                    }
                }
            })
        }

        // Agent commands (need tokio runtime for socket communication)
        Command::Agent { action } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                match *action {
                    AgentAction::Start(args) => {
                        let proj = args
                            .project
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        commands::agent::start(wisphive_protocol::SpawnAgentRequest {
                            project: proj,
                            prompt: args.prompt,
                            model: args.model,
                            name: args.name,
                            reasoning: args.reasoning,
                            max_turns: args.max_turns,
                            permission_mode: args.permission_mode,
                            system_prompt: args.system_prompt,
                            append_system_prompt: args.append_system_prompt,
                            allowed_tools: args.allowed_tools,
                            disallowed_tools: args.disallowed_tools,
                            continue_session: args.continue_session,
                            resume: args.resume,
                            output_format: args.output_format,
                            verbose: args.verbose,
                        })
                        .await
                    }
                    AgentAction::List => commands::agent::list().await,
                    AgentAction::Stop { agent_id } => commands::agent::stop(agent_id).await,
                }
            })
        }

        // Daemon-dependent commands (need tokio runtime)
        Command::Daemon { action } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                match action {
                    DaemonAction::Start {
                        web,
                        host,
                        port,
                        web_dev,
                        no_open,
                        auth_profile,
                        auth_rp_id,
                    } => {
                        // Any of --web / non-default --host / non-default --port / --web-dev
                        // implies "serve the web UI too".
                        let web_requested = web || web_dev || host != "127.0.0.1" || port != 3100;
                        let web_opts = if web_requested {
                            // Use the same parser as `web serve` — including
                            // the `0.0.0.0` LAN-exposure WARNING. Prior to
                            // this the daemon-start path silently accepted
                            // --host 0.0.0.0 while `web serve` warned, a
                            // behavioral regression flagged in the itr#215
                            // efficiency review (eff#1).
                            let Some(host_octets) = parse_host_octets(&host) else {
                                return Ok(());
                            };
                            // itr#310: resolve the auth profile *before*
                            // launching the daemon. Enterprise fail-fast
                            // exits here with a clean stderr message so
                            // operators don't end up with a half-bootstrapped
                            // daemon they have to kill.
                            let profile =
                                match resolve_auth_profile(auth_profile, auth_rp_id.as_deref()) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        eprintln!("{e}");
                                        return Ok(());
                                    }
                                };
                            // Dev mode stays http (Vite serves the UI over
                            // http and dragging the user through self-signed
                            // TLS isn't worth it); prod is https, and the
                            // banner enumerates every LAN URL + fingerprint.
                            let home = std::env::var("HOME")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                                .join(".wisphive");
                            print_startup_banner(&home, host_octets, port, web_dev);
                            Some(commands::daemon::WebOptions {
                                host: host_octets,
                                port,
                                dev: web_dev,
                                no_open,
                                auth_profile: profile,
                            })
                        } else {
                            None
                        };
                        commands::daemon::start(web_opts).await
                    }
                    DaemonAction::Stop => commands::daemon::stop().await,
                    DaemonAction::Status => commands::daemon::status().await,
                }
            })
        }
        Command::Term { action } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async move {
                match action {
                    TermAction::New {
                        label,
                        cwd,
                        cmd,
                        arg,
                        attach,
                    } => {
                        let args = if arg.is_empty() { None } else { Some(arg) };
                        commands::term::new_session(label, cwd, cmd, args, attach).await
                    }
                    TermAction::List => commands::term::list().await,
                    TermAction::Attach { id } => commands::term::attach(id).await,
                    TermAction::Replay { id, speed } => commands::term::replay(id, speed).await,
                    TermAction::Close { id, kill } => commands::term::close(id, kill).await,
                }
            })
        }
        Command::Tui => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(commands::tui::run())
        }
        Command::Web { action } => match action {
            WebAction::Serve {
                port,
                host,
                dev,
                no_open,
                auth_profile,
                auth_rp_id,
            } => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(serve_web(
                    port,
                    host,
                    dev,
                    no_open,
                    auth_profile,
                    auth_rp_id,
                ))
            }
            WebAction::SetPassword => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(commands::web::set_password())
            }
            WebAction::ResetPassword => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(commands::web::reset_password())
            }
            WebAction::Devices { action } => {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(async move {
                    match action {
                        DevicesAction::List => commands::web::devices_list().await,
                        DevicesAction::Revoke { id } => commands::web::devices_revoke(id).await,
                    }
                })
            }
            WebAction::Fingerprint => commands::web::fingerprint(),
        },
    }
}

/// Resolve the CLI's [`AuthProfileArg`] + optional `--auth-rp-id` flag
/// into a concrete [`wisphive_web::AuthProfile`] (itr#310).
///
/// LocalLAN is unconditional. Enterprise is fail-fast: it requires a real
/// registrable domain in `--auth-rp-id` AND (per the itr#270 dependency)
/// user-provided TLS cert + key. Until itr#270 ships the `--tls-cert` /
/// `--tls-key` flags, Enterprise selection always fails with a clear
/// error pointing operators at the missing prerequisite.
///
/// Returns the error message ready to print to stderr — callers just need
/// to exit cleanly without running the daemon.
fn resolve_auth_profile(
    arg: AuthProfileArg,
    auth_rp_id: Option<&str>,
) -> Result<wisphive_web::AuthProfile, String> {
    match arg {
        AuthProfileArg::LocalLan => {
            if auth_rp_id.is_some() {
                // Not an error — just noisy. Warn so operators don't
                // think the flag silently took effect.
                eprintln!(
                    "warning: --auth-rp-id is ignored under --auth-profile local-lan \
                     (LocalLAN uses RP ID 'localhost' for loopback origins and \
                     no RP ID for LAN-IP origins)"
                );
            }
            Ok(wisphive_web::AuthProfile::LocalLAN)
        }
        AuthProfileArg::Enterprise => {
            // itr#270 dep: --tls-cert / --tls-key don't exist yet. We
            // hard-code `false`/`false` so the validator emits the
            // documented MissingTlsFlags message. Once #270 lands, the
            // CLI plumbs those flags and passes their `is_some()` status
            // here.
            let tls_cert_provided = false;
            let tls_key_provided = false;
            let rp_id = wisphive_web::auth_profile::validate_enterprise_config(
                auth_rp_id,
                tls_cert_provided,
                tls_key_provided,
            )
            .map_err(|e| e.to_string())?;
            // RP origin is derived from the RP ID + https scheme — a
            // conservative default that matches the "user brings a real
            // domain" Enterprise contract. #270 will refine this once
            // the cert's primary SAN is known.
            let rp_origin = wisphive_web::Url::parse(&format!("https://{}", rp_id.as_str()))
                .map_err(|e| {
                    format!("internal: failed to derive rp_origin from --auth-rp-id: {e}")
                })?;
            Ok(wisphive_web::AuthProfile::Enterprise { rp_id, rp_origin })
        }
    }
}

/// Parse a dotted-quad IPv4 string into a `[u8; 4]` octet array. Shared by
/// `wisphive web serve` and `wisphive daemon start --web`.
///
/// Returns `None` on invalid input — the error is already printed to
/// stderr, so callers just need to exit cleanly. Prints a WARNING on
/// `0.0.0.0` so operators notice the LAN exposure.
fn parse_host_octets(host: &str) -> Option<[u8; 4]> {
    match host {
        "0.0.0.0" => {
            eprintln!(
                "WARNING: Web UI is exposed on all network interfaces. Ensure this is intentional."
            );
            Some([0, 0, 0, 0])
        }
        "127.0.0.1" | "localhost" => Some([127, 0, 0, 1]),
        other => {
            let parts: Vec<u8> = other.split('.').filter_map(|s| s.parse().ok()).collect();
            if parts.len() == 4 {
                Some([parts[0], parts[1], parts[2], parts[3]])
            } else {
                eprintln!("Invalid host address: {other}");
                None
            }
        }
    }
}

async fn serve_web(
    port: u16,
    host: String,
    dev: bool,
    no_open: bool,
    auth_profile: AuthProfileArg,
    auth_rp_id: Option<String>,
) -> anyhow::Result<()> {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join(".wisphive");
    let socket_path = home.join("wisphive.sock");

    let Some(host_octets) = parse_host_octets(&host) else {
        return Ok(());
    };

    // itr#310: resolve the auth profile up front so an Enterprise misconfig
    // fails before we start the listener.
    let profile = match resolve_auth_profile(auth_profile, auth_rp_id.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return Ok(());
        }
    };

    print_startup_banner(&home, host_octets, port, dev);

    // itr#267: if this is a first-run install, pop the default browser
    // onto the SPA once the server is actually listening. The SPA decides
    // between the Login and Onboarding screens based on /api/auth/status,
    // so we always open the root `/` — no `?token=` in the URL, since the
    // per-process web.token bootstrap was retired in itr#213.
    let browser_task = if no_open {
        None
    } else {
        Some(tokio::spawn(maybe_open_browser(
            home.join("wisphive.db"),
            host_octets,
            port,
            dev,
        )))
    };

    let result = wisphive_web::serve(socket_path, port, dev, host_octets, profile).await;
    if let Some(h) = browser_task {
        h.abort();
    }
    result
}

/// Build the user-facing URL for the web UI and (on first-run) open it in
/// the default browser. Intentionally a no-op on failure: a missing default
/// browser or a sandboxed CI runner should log a warning and move on, never
/// panic or block startup.
///
/// Called for `wisphive web serve` and `wisphive daemon start --web`. The
/// caller MUST have already ensured `no_open` is false before spawning
/// this task.
pub(crate) async fn maybe_open_browser(
    db_path: std::path::PathBuf,
    host: [u8; 4],
    port: u16,
    dev: bool,
) {
    // Small delay so the axum/axum-server `bind` has a chance to land
    // before we point a browser at the port. Without this the first tab
    // races the bind and usually reloads once — ugly but not fatal.
    // 400ms is enough in practice and still feels instant to the human.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    // Open the same DB the web server will — WAL + connection pooling
    // makes this safe. We hold the handle only for the first-run check.
    let db = match wisphive_daemon::state::StateDb::open(db_path.to_string_lossy().as_ref()).await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(error = %e, "first-run browser: failed to open state DB; skipping");
            return;
        }
    };
    if !wisphive_web::auth::is_first_run(&db).await {
        return;
    }

    // Always auto-open against loopback. A specific `--host <lan-ip>` bind
    // would otherwise produce a URL whose hostname isn't in the self-signed
    // TLS cert's SAN list (only `127.0.0.1` + `::1` + `localhost` are there
    // today; LAN IP SANs land with itr#270) — the tab would greet the
    // operator with a cert-hostname-mismatch error instead of the setup
    // screen. 127.0.0.1 is guaranteed to route to this machine, is always
    // in the SAN, and the startup banner already prints the LAN URL for
    // manual copy when the operator does want to browse from elsewhere.
    let host_for_url = "127.0.0.1";
    let _ = host; // bind still dictates listener; kept for future LAN-aware open
    let scheme = if dev { "http" } else { "https" };
    let url = format!("{scheme}://{host_for_url}:{port}/");
    // Defense-in-depth against future ?query= being added: the URL must
    // stay in an alphabet that's safe to pass to platform-specific
    // openers (macOS `open`, Linux `xdg-open`, Windows `cmd /C start`).
    // Today's format can't produce anything else, but asserting makes
    // the invariant load-bearing.
    debug_assert!(
        url.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".:/-".contains(c)),
        "auto-open URL contains unexpected characters: {url}"
    );

    match open::that_detached(&url) {
        Ok(_) => tracing::info!(%url, "first-run: opened browser to Wisphive setup"),
        Err(e) => tracing::warn!(
            %url,
            error = %e,
            "first-run: failed to open default browser; visit the URL manually"
        ),
    }
}

/// Emit the pre-serve banner: scheme + bind + every LAN URL the TLS cert
/// covers + the on-disk fingerprint (in prod). Replaces the old
/// `ipconfig getifaddr en0` probe, which only found the first wired/WiFi
/// interface on macOS and returned nothing on Linux.
///
/// The banner is best-effort: a missing fingerprint (first-run before
/// `ensure_cert` has run) or `enumerate_lan_urls` empty list should not
/// prevent the server from starting. Errors go to stderr with a note, not
/// a bail.
fn print_startup_banner(home: &std::path::Path, host_octets: [u8; 4], port: u16, dev: bool) {
    if dev {
        let host_str = if host_octets == [0, 0, 0, 0] {
            "0.0.0.0".to_string()
        } else {
            format!(
                "{}.{}.{}.{}",
                host_octets[0], host_octets[1], host_octets[2], host_octets[3]
            )
        };
        eprintln!("Wisphive Web (dev mode)");
        eprintln!("  WebSocket: http://{host_str}:{port}/ws");
        eprintln!("  Run `cd crates/wisphive_web/frontend && npm run dev` for the UI");
        return;
    }

    eprintln!("Wisphive Web (TLS):");
    let bind_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::from(host_octets));
    for url in wisphive_web::tls::enumerate_lan_urls(bind_ip, port) {
        eprintln!("  {url}");
    }
    match wisphive_web::tls::read_cert_fingerprint(home) {
        Ok(Some(fp)) => eprintln!("  fingerprint: {fp}"),
        Ok(None) => eprintln!("  fingerprint: (cert will be minted on first request)"),
        Err(e) => eprintln!("  fingerprint: (read error: {e})"),
    }
}
