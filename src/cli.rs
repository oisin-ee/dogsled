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
    /// Publish local measurements, reconcile owned work, and admit queued actions.
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
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value = "30m", value_parser = crate::jobs::duration)]
        max: u64,
    },
    /// Place a command in a Herdr tab, or queue it until capacity is available.
    Run {
        #[command(flatten)]
        placement: WorkArgs,
        #[arg(long, value_parser = crate::jobs::duration)]
        timeout: Option<u64>,
        #[arg(long, value_parser = crate::jobs::duration)]
        idle_timeout: Option<u64>,
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Execute a job inside its pane.
    Exec {
        id: String,
        #[arg(long, value_parser = crate::jobs::duration)]
        timeout: Option<u64>,
        #[arg(long, value_parser = crate::jobs::duration)]
        idle_timeout: Option<u64>,
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// List jobs across the fleet.
    Ps {
        #[arg(long)]
        all: bool,
    },
    Logs {
        id: String,
        #[arg(long)]
        follow: bool,
    },
    Stop {
        id: String,
    },
    Wait {
        id: String,
        #[arg(long, default_value = "30m", value_parser = crate::jobs::duration)]
        max: u64,
    },
    /// Start any harness supported by Herdr.
    Agent {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        message: String,
        #[command(flatten)]
        placement: WorkArgs,
    },
    Track {
        host: String,
        pane: String,
        #[arg(long)]
        name: Option<String>,
    },
    Items {
        #[arg(long)]
        owner: Option<String>,
    },
    Cancel {
        ticket: String,
    },
    /// Internal local operations used by peers over SSH.
    #[command(subcommand, hide = true)]
    Host(HostCommand),
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    Snapshot,
    Launch {
        #[arg(long)]
        request: String,
    },
    Job {
        id: String,
    },
    Jobs,
    Stop {
        id: String,
    },
    Logs {
        id: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    Checkout {
        #[arg(long)]
        path: String,
        #[arg(long)]
        origin: String,
        #[arg(long)]
        host_label: String,
    },
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

#[derive(Debug, Args)]
pub struct WorkArgs {
    #[arg(long)]
    pub host: Option<String>,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long, requires = "branch")]
    pub base: Option<String>,
    #[arg(long)]
    pub cwd: Option<String>,
    #[arg(long)]
    pub wait: bool,
    #[arg(long, default_value = "30m", value_parser = crate::jobs::duration)]
    pub max: u64,
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
