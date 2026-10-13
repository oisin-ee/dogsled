//! Public commands and local primitives used by the SSH transport.

use crate::config::Thresholds;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "wrangle",
    version,
    about = "Host status and placement for Herdr agents",
    propagate_version = true
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Collect this host's measurements. Never uses SSH.
    Serve {
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        #[arg(long)]
        once: bool,
    },
    /// Read all hosts' snapshots, with a shared 10-second peer cache.
    Status {
        #[arg(long)]
        watch: bool,
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        #[arg(long, conflicts_with_all = ["brief", "json"])]
        line: bool,
        #[arg(long, conflicts_with = "json")]
        brief: bool,
    },
    /// Reserve capacity on the host with the most headroom.
    Place {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long, requires = "branch")]
        base: Option<String>,
        /// Repository directory. Defaults to the current directory.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Internal local operations used by peers over SSH.
    #[command(subcommand, hide = true)]
    Host(HostCommand),
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    Snapshot,
    Reserve {
        #[arg(long)]
        id: String,
        #[command(flatten)]
        thresholds: ThresholdArgs,
    },
    Prepare {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        branch: String,
        #[arg(long)]
        base: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Args)]
pub struct ThresholdArgs {
    #[arg(long)]
    pub load_per_core_max: Option<f64>,
    #[arg(long)]
    pub disk_free_min_percent: Option<f64>,
    #[arg(long)]
    pub reservation_load: Option<f64>,
    #[arg(long)]
    pub reservation_ttl_ms: Option<u64>,
}
impl ThresholdArgs {
    pub fn over(self, base: Thresholds) -> Thresholds {
        Thresholds {
            load_per_core_max: self.load_per_core_max.unwrap_or(base.load_per_core_max),
            disk_free_min_percent: self
                .disk_free_min_percent
                .unwrap_or(base.disk_free_min_percent),
            reservation_load: self.reservation_load.unwrap_or(base.reservation_load),
            reservation_ttl_ms: self.reservation_ttl_ms.unwrap_or(base.reservation_ttl_ms),
        }
    }
}
