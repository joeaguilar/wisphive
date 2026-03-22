mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "wisphive",
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

    /// Launch the Web UI server
    Web {
        /// HTTP port (default: 3100)
        #[arg(short, long, default_value = "3100")]
        port: u16,
        /// Dev mode: only serve WebSocket, expect Vite dev server for frontend
        #[arg(long)]
        dev: bool,
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
        action: AgentAction,
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
    /// Start the background daemon
    Start,
    /// Stop the running daemon
    Stop,
    /// Show daemon status
    Status,
}

#[derive(Subcommand)]
enum AgentAction {
    /// Start an AI agent in a project directory
    Start {
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
    },
    /// List running agent processes
    List,
    /// Stop a running agent process
    Stop {
        /// Agent ID to stop
        agent_id: String,
    },
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
                match action {
                    AgentAction::Start {
                        project,
                        prompt,
                        model,
                        name,
                        reasoning,
                        max_turns,
                        permission_mode,
                        system_prompt,
                        append_system_prompt,
                        allowed_tools,
                        disallowed_tools,
                        continue_session,
                        resume,
                        output_format,
                        verbose,
                    } => {
                        let proj = project
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        commands::agent::start(wisphive_protocol::SpawnAgentRequest {
                            project: proj,
                            prompt,
                            model,
                            name,
                            reasoning,
                            max_turns,
                            permission_mode,
                            system_prompt,
                            append_system_prompt,
                            allowed_tools,
                            disallowed_tools,
                            continue_session,
                            resume,
                            output_format,
                            verbose,
                        }).await
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
                    DaemonAction::Start => commands::daemon::start().await,
                    DaemonAction::Stop => commands::daemon::stop().await,
                    DaemonAction::Status => commands::daemon::status().await,
                }
            })
        }
        Command::Tui => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(commands::tui::run())
        }
        Command::Web { port, dev } => {
            let rt = tokio::runtime::Runtime::new()?;
            let home = std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
                .join(".wisphive");
            let socket_path = home.join("wisphive.sock");
            if dev {
                eprintln!("Wisphive Web (dev mode)");
                eprintln!("  WebSocket: http://127.0.0.1:{port}/ws");
                eprintln!("  Run `cd crates/wisphive_web/frontend && npm run dev` for the UI");
            } else {
                eprintln!("Wisphive Web: http://127.0.0.1:{port}");
            }
            rt.block_on(wisphive_web::serve(socket_path, port, dev))
        }
    }
}
