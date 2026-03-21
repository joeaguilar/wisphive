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

    /// Check setup and diagnose issues
    Doctor {
        /// Project directory to check (defaults to current directory)
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
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
        Command::Doctor { project } => commands::doctor::run(project),
        Command::EmergencyOff => commands::hooks::emergency_off(),
        Command::Hooks { action } => match action {
            HookAction::Enable => commands::hooks::set_mode("active"),
            HookAction::Disable => commands::hooks::set_mode("off"),
            HookAction::Install { project, all } => commands::hooks::install(project, all),
            HookAction::Uninstall { project, all } => commands::hooks::uninstall(project, all),
            HookAction::Status => commands::hooks::status(),
        },

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
    }
}
