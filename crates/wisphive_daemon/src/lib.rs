pub mod config;
pub mod config_watch;
pub mod disk_alert;
pub mod event_ingest;
pub mod hook_install;
pub mod logging;
pub mod notify;
pub mod process_registry;
pub mod project_audit;
pub mod queue;
pub mod registry;
pub mod replay_gate;
pub mod server;
pub mod shutdown;
pub mod state;
pub mod sudo_gate;
pub mod terminal;

pub use config::{
    ConfigUpdateError, DaemonConfig, UserConfig, update_config_json, write_config_atomic,
};
