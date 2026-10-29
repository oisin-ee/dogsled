//! Public commands and local primitives used by the SSH transport.

use crate::config::Thresholds;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "dogsled",
    version,
    about = "Host status and placement for Herdr agents",
    propagate_version = true
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Remaining caller budget propagated across SSH.
    #[arg(long, global = true, hide = true)]
    pub budget_ms: Option<u64>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Read quota windows from the collector or the local snapshot.
    Quota(QuotaArgs),
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
    /// What an owner started, grouped by what the owner must do next.
    Board {
        /// The owner's Herdr pane. Defaults to the caller's; `--all` shows every owner.
        #[arg(long, conflicts_with = "all")]
        owner: Option<String>,
        #[arg(long)]
        all: bool,
        /// Open or reuse this tab's board, docked on the left.
        #[arg(long, conflicts_with_all = ["all", "todo_path", "toggle"])]
        open: bool,
        /// Close this tab's docked board, or open it when there is none.
        #[arg(long, conflicts_with_all = ["all", "todo_path"])]
        toggle: bool,
        /// Local todo publication path.
        #[arg(long, conflicts_with = "all")]
        todo_path: bool,
    },
    /// The dock pane: framed Agents & Jobs and Todos panels. Herdr runs it; it needs a terminal.
    Sidebar {
        /// The owner's Herdr pane. Defaults to the caller's.
        #[arg(long)]
        owner: Option<String>,
    },
    /// Serve the portal's board API on the loopback address.
    Web {
        /// The port to listen on. Defaults to the configured port.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Open one interview on a temporary loopback server and print its answers.
    Interview {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value = "")]
        pane: String,
        #[arg(long, default_value = "")]
        session: String,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 1200)]
        timeout: u64,
    },
    /// Open one plan review on a temporary loopback server and print its decision.
    Review {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        pane: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        cwd: String,
    },
    /// List cleanup candidates without changing them.
    Doctor {
        #[arg(long)]
        host: Option<String>,
    },
    /// Show and confirm a safe cleanup plan.
    Prune {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(value_parser = ["worktrees", "branches", "workspaces", "panes", "jobs", "targets", "disk", "processes"])]
        classes: Vec<String>,
    },
    /// Reserve capacity on the host with the most headroom.
    Place {
        #[arg(long, conflicts_with_all = ["host", "branch", "base", "repo"])]
        resume: Option<String>,
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
        /// The job validity protocol the launch used.
        #[arg(long)]
        job_validity: Option<u32>,
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
    /// Start a Pi, Claude, or Codex child with its task. With `--profile`, a Pi child gets the
    /// profile's model, thinking, and system prompt from the host it runs on.
    Agent {
        #[arg(long, default_value = "pi", value_parser = crate::work::KINDS)]
        kind: String,
        #[arg(long)]
        name: String,
        /// The task. Use `--message-file` for a long or multi-line task.
        #[arg(
            long,
            required_unless_present = "message_file",
            conflicts_with = "message_file"
        )]
        message: Option<String>,
        /// A file with the task, or `-` for stdin.
        #[arg(long, value_name = "PATH")]
        message_file: Option<PathBuf>,
        /// A profile in `~/.config/dogsled/profiles`; Pi only.
        #[arg(long)]
        profile: Option<String>,
        #[command(flatten)]
        placement: WorkArgs,
    },
    /// Durable unit request protocol. All operations accept and return JSON.
    #[command(subcommand)]
    Unit(UnitCommand),
    /// List the agent profiles on this host.
    Profiles,
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
    /// Close a finished child: run its worktree's `teardown`, close the panes and Herdr workspace
    /// that use it, remove the worktree, and end its item. Refuses busy or dirty work.
    Close {
        /// The child's Herdr pane (the spawn result's `target`).
        pane: String,
        /// The child's host. Defaults to the host of the open item for this pane, else local.
        #[arg(long)]
        host: Option<String>,
    },
    /// Claim custody of an orphaned child on this host.
    Claim {
        item: String,
        #[arg(long)]
        revision: Option<u64>,
    },
    /// Accept an exact returned result without completing its original task.
    Accept {
        item: String,
        #[arg(long)]
        result: String,
        #[arg(long)]
        evidence: String,
    },
    /// Delete a closed child's retained branch, including its origin branch.
    Drop {
        /// The closed agent item's id.
        id: String,
        #[arg(long)]
        host: Option<String>,
    },
    Cancel {
        ticket: String,
    },
    /// Say that this agent's whole task is complete. Call it during the turn that completes the
    /// task; dogsled closes the child after that turn ends and keeps its branch. Without
    /// it, an idle child waits for input and stays open. Refused while it owns open children.
    Done,
    /// Ask your owner a question without completing the task.
    Ask {
        question: String,
    },
    /// Report why you cannot continue without completing the task.
    Blocked {
        reason: String,
    },
    /// Show a child's or job's newest output and its captured final reply.
    Read {
        /// A child's name or pane, a job id, or any agent's name or pane.
        target: String,
        #[arg(long)]
        host: Option<String>,
    },
    /// Serve the dogsled tools to a harness over MCP on stdio.
    Mcp,
    /// Read an unreachable host again now, without waiting out its backoff.
    Retry {
        host: String,
    },
    /// The saved Herdr machines dogsled uses as hosts.
    #[command(subcommand)]
    Machine(MachineCommand),
    /// Send a message to an agent on any host, through its inbox. Nothing types into its
    /// terminal; an agent without an inbox reader gets nothing, and you hear so at once.
    Send {
        /// The agent's name or Herdr pane.
        target: String,
        #[arg(long)]
        host: Option<String>,
        /// A file with the message, or `-` for stdin.
        #[arg(long, value_name = "PATH", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        /// The message text.
        #[arg(long, required_unless_present = "message_file")]
        message: Option<String>,
        /// `message` for information, `task` for work to do.
        #[arg(long, default_value = "message", value_parser = ["message", "task"])]
        kind: String,
    },
    /// Continue, as its owner, a seeded worker on this host that called `done` before its unit
    /// returned and whose launch record predates the owner host. `send` refuses such a worker.
    /// The original admission must be on this host, and you must run the session that started
    /// the unit.
    LegacyContinue {
        /// The worker's Herdr pane on this host.
        pane: String,
        /// A file with the message, or `-` for stdin.
        #[arg(long, value_name = "PATH", conflicts_with = "message")]
        message_file: Option<PathBuf>,
        /// The message text.
        #[arg(long, required_unless_present = "message_file")]
        message: Option<String>,
        /// `message` for information, `task` for work to do.
        #[arg(long, default_value = "message", value_parser = ["message", "task"])]
        kind: String,
    },
    /// Read and acknowledge this agent's inbox.
    #[command(subcommand)]
    Inbox(InboxCommand),
    /// Add host status to a harness prompt without blocking it on failure.
    #[command(subcommand)]
    Hook(HookCommand),
    /// Internal local operations used by peers over SSH.
    #[command(subcommand, hide = true)]
    Host(HostCommand),
}

#[derive(Debug, Args)]
pub struct QuotaArgs {
    /// Never access Keychain or the network.
    #[arg(long, conflicts_with_all = ["refresh", "store"])]
    pub cached: bool,
    /// Refresh even when the snapshot is younger than five minutes.
    #[arg(long, conflicts_with = "store")]
    pub refresh: bool,
    /// Receive a validated snapshot on stdin.
    #[arg(long, hide = true)]
    pub store: bool,
}

#[derive(Debug, Subcommand)]
pub enum UnitCommand {
    /// Uncommitted requests for this owner, for confirmed runtime-boundary recovery.
    Prepared,
    Prepare {
        #[arg(long)]
        input: String,
        /// Originating adapter invocation. Retained with the request, not a lease.
        #[arg(long)]
        tool_call_id: String,
    },
    Commit {
        #[arg(long)]
        input: String,
    },
    Observe {
        #[arg(long)]
        input: String,
    },
    AbandonPrepared {
        #[arg(long)]
        input: String,
    },
    /// Worker-host acknowledgement of a sealed execution and final reply boundary.
    TerminalAck {
        #[arg(long)]
        input: String,
    },
    /// Pending task settlements for this owner; never consumes transcript delivery.
    Settlements,
    /// Record provider success or confirmed duplicate application.
    AckSettlement {
        #[arg(long)]
        input: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum MachineCommand {
    /// Every saved machine, enabled or not.
    List,
    Rename {
        machine: String,
        label: String,
    },
    Enable {
        machine: String,
    },
    /// Stop using a machine as a host. Herdr keeps it saved.
    Disable {
        machine: String,
    },
    /// Authenticate in a new tab, for the user to answer SSH prompts.
    Reconnect {
        machine: String,
    },
    /// Save an SSH machine; runs `herdr machine add` in a new tab.
    Add {
        target: String,
        #[arg(long)]
        label: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// Register as this pane's inbox reader and claim its deliverable records. A claimed
    /// record is offered again, with backoff, until it is acknowledged.
    Claim {
        /// The reading harness: pi, claude, or codex.
        #[arg(long)]
        harness: String,
        /// Defaults to `HERDR_PANE_ID`.
        #[arg(long)]
        pane: Option<String>,
    },
    /// Mark claimed records delivered.
    Ack {
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        pane: Option<String>,
    },
    /// Records this pane received and sent, with their states.
    List {
        #[arg(long)]
        pane: Option<String>,
    },
    /// The directory and file name of this pane's inbox, for a reader that watches it.
    Path {
        #[arg(long)]
        pane: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub enum HookCommand {
    /// Remind the owner once about finished children with work to land (plain text for Pi).
    Gate,
    Context {
        #[arg(long)]
        harness: crate::context::Harness,
        /// The lifecycle event registered by the plugin; stdin is never read.
        #[arg(long, default_value = "UserPromptSubmit")]
        event: crate::context::Event,
    },
    /// Claim this pane's inbox for a Claude Code or Codex hook, print the records in the
    /// event's output format, and acknowledge them. Stop also reminds about unlanded children.
    Inbox {
        #[arg(long)]
        harness: crate::context::Harness,
        /// `Stop` continues the turn with the records as the next prompt; `UserPromptSubmit`
        /// adds them as context beside the user's own text.
        #[arg(long)]
        event: crate::inbox::HookEvent,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    Snapshot,
    /// Delete an owner-authorized closed item's branch on this host.
    Drop {
        #[arg(long)]
        item: String,
    },
    /// Refresh retained branches in one host call.
    RefreshBranches {
        #[arg(long)]
        items: String,
    },
    /// SSH transport for quota --store.
    QuotaStore,
    /// Read the service's rendered todo snapshot; no task state.
    Todos,
    /// Put one inbox record, sent from another host.
    InboxPut {
        #[arg(long)]
        record: String,
    },
    /// The states of records in one terminal's inbox.
    InboxState {
        terminal: String,
        ids: Vec<String>,
    },
    Parent {
        pane: String,
    },
    Tail {
        kind: String,
        id: String,
    },
    /// Read immutable seeded terminal evidence on its execution host.
    UnitReceipt {
        #[arg(long)]
        input: String,
    },
    /// Read the seeded terminal evidence of many executions on this host in one call.
    UnitReceipts {
        #[arg(long)]
        input: String,
    },
    /// The final reply of the finished agent in a pane.
    Reply {
        pane: String,
    },
    Doctor,
    Prune {
        #[arg(long)]
        plan: String,
    },
    Close {
        pane: String,
        /// Legacy transport spelling for an orphan handoff: require done and keep its branch.
        #[arg(long)]
        pushed: bool,
        /// Correlated stopped-orphan episode; the host verifies its current session and turn.
        #[arg(long, conflicts_with_all = ["pushed", "landed", "writer"])]
        stopped_orphan: Option<String>,
        /// Legacy transport spelling for an owned handoff: require done and keep its branch.
        #[arg(long)]
        landed: bool,
        #[arg(long, requires = "landed")]
        writer: bool,
    },
    Launch {
        #[arg(long)]
        request: String,
        #[arg(long)]
        preparation_protocol: Option<u32>,
        /// The job validity protocol the caller speaks. A job launch carries it.
        #[arg(long)]
        job_validity: Option<u32>,
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
        /// Versioned placement expectation; never falls back to TTL-only admission.
        #[arg(long, conflicts_with_all = ["job_validity", "job_created_ms"])]
        placement_replay: Option<String>,
        #[arg(long)]
        preparation_protocol: Option<u32>,
        /// The job validity protocol the caller speaks. A job reservation carries it.
        #[arg(long)]
        job_validity: Option<u32>,
        /// The recorded creation time of the request this reservation is for.
        #[arg(long)]
        job_created_ms: Option<u64>,
        #[command(flatten)]
        thresholds: ThresholdArgs,
    },
    PreparationLookup {
        #[arg(long)]
        input: String,
        #[arg(long)]
        preparation_protocol: u32,
    },
    Prepared {
        #[arg(long)]
        input: String,
        #[arg(long)]
        preparation_protocol: u32,
    },
    Prepare {
        #[arg(long)]
        preparation_protocol: Option<u32>,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        branch: String,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
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
    pub free_core_fraction_min: Option<f64>,
    #[arg(long)]
    pub memory_reserve_bytes: Option<u64>,
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
            free_core_fraction_min: self
                .free_core_fraction_min
                .unwrap_or(base.free_core_fraction_min),
            memory_reserve_bytes: self
                .memory_reserve_bytes
                .unwrap_or(base.memory_reserve_bytes),
            disk_free_min_percent: self
                .disk_free_min_percent
                .unwrap_or(base.disk_free_min_percent),
            reservation_load: self.reservation_load.unwrap_or(base.reservation_load),
            reservation_ttl_ms: self.reservation_ttl_ms.unwrap_or(base.reservation_ttl_ms),
        }
    }
}
