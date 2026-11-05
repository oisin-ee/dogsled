//! Typed host-owned state. Local facts have columns and ordered child tables.
//! Exact peer and identity evidence has named message columns and is never queried inside.

mod command;
mod peer_rows;
pub(crate) const PEER_SCHEMA: &str = peer_rows::SCHEMA;
#[cfg(test)]
mod tests;
pub(crate) use command::{Command, fault, run};

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::rows::{self, Columns};
use rusqlite::types::Value as Sql;
use rusqlite::{Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

fn invalid(text: &str) -> Error {
    Error::Invalid(text.into())
}
fn path_text(path: &std::path::Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("stored path is not UTF-8"))
}
fn finite(value: f64) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(invalid("stored number is not finite"))
    }
}
fn number<T: TryFrom<i64>>(row: &Row<'_>, column: &str) -> Result<T> {
    T::try_from(row.get::<_, i64>(column)?).map_err(|_| invalid("stored integer out of range"))
}
fn optional_number<T: TryFrom<i64>>(row: &Row<'_>, column: &str) -> Result<Option<T>> {
    rows::optional(row.get::<_, Option<i64>>(column)?, |value| {
        T::try_from(value).map_err(|_| invalid("stored integer out of range"))
    })
}

pub(crate) const SCHEMA:&str=concat!(
    "CREATE TABLE host_sweep_request (id TEXT PRIMARY KEY CHECK(id = 'local'), token TEXT NOT NULL CHECK(length(token) > 0)) STRICT;
    CREATE TABLE host_gate (id TEXT PRIMARY KEY CHECK(length(id) > 0)) STRICT;
    CREATE TABLE host_gate_sets (owner TEXT NOT NULL REFERENCES host_gate(id) ON DELETE CASCADE, position INTEGER NOT NULL CHECK(position >= 0), PRIMARY KEY(owner,position)) STRICT;
    CREATE TABLE host_gate_entries (owner TEXT NOT NULL, signature_position INTEGER NOT NULL, position INTEGER NOT NULL CHECK(position >= 0), item TEXT NOT NULL, ahead INTEGER CHECK(ahead >= 0), dirty INTEGER NOT NULL CHECK(dirty IN (0,1)), PRIMARY KEY(owner,signature_position,position), FOREIGN KEY(owner,signature_position) REFERENCES host_gate_sets(owner,position) ON DELETE CASCADE) STRICT;
    " ,
r#"CREATE TABLE host_snapshot_probe_agents_by_status (
 owner TEXT NOT NULL REFERENCES host_snapshot(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 value INTEGER NOT NULL CHECK (value >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_snapshot_probe_agents_by_status_owner ON host_snapshot_probe_agents_by_status(owner,position);"#,
r#"CREATE TABLE host_snapshot_probe_agent_panes (
 owner TEXT NOT NULL REFERENCES host_snapshot(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 pane TEXT NOT NULL,
 agent TEXT,
 state TEXT NOT NULL,
 name TEXT,
 transcript TEXT,
 transcript_modified INTEGER CHECK (transcript_modified >= 0),
 stalled_since_ms INTEGER CHECK (stalled_since_ms >= 0),
 state_since_ms INTEGER CHECK (state_since_ms >= 0),
 signal TEXT,
 terminal_id TEXT,
 agent_session TEXT,
 completion_seq INTEGER CHECK (completion_seq >= 0),
 state_change_seq INTEGER CHECK (state_change_seq >= 0),
 cwd TEXT,
 session_model TEXT,
 session_response_model TEXT,
 session_thinking TEXT,
 session_activity TEXT,
 worktree_present INTEGER NOT NULL CHECK(worktree_present IN (0,1)),
 worktree_path TEXT,
 worktree_branch TEXT,
 worktree_ahead INTEGER CHECK (worktree_ahead >= 0),
 worktree_dirty INTEGER CHECK (worktree_dirty IN (0,1)),
 worktree_landed INTEGER CHECK (worktree_landed IN (0,1)),
 worktree_head TEXT,
 worktree_base TEXT,
 repository TEXT,
 done INTEGER NOT NULL CHECK (done IN (0,1)),
 finishing INTEGER NOT NULL CHECK (finishing IN (0,1)),
 handoff TEXT,
 launch_progress TEXT,
 owns_open INTEGER NOT NULL CHECK (owns_open IN (0,1)),
 lifecycle TEXT,
 lifecycle_gap TEXT,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_snapshot_probe_agent_panes_owner ON host_snapshot_probe_agent_panes(owner,position);"#,
r#"CREATE TABLE host_snapshot_probe_reservations (
 owner TEXT NOT NULL REFERENCES host_snapshot(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 id TEXT NOT NULL,
 created_ms INTEGER NOT NULL CHECK (created_ms >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_snapshot_probe_reservations_owner ON host_snapshot_probe_reservations(owner,position);"#,
r#"CREATE TABLE host_snapshot_probe_panes (
 owner TEXT NOT NULL REFERENCES host_snapshot(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 pane_id TEXT NOT NULL,
 focused INTEGER NOT NULL CHECK (focused IN (0,1)),
 workspace_id TEXT,
 name TEXT,
 agent TEXT,
 agent_status TEXT,
 cwd TEXT,
 foreground_cwd TEXT,
 idle_shell INTEGER NOT NULL CHECK (idle_shell IN (0,1)),
 idle_since_ms INTEGER CHECK (idle_since_ms >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_snapshot_probe_panes_owner ON host_snapshot_probe_panes(owner,position);"#,
r#"CREATE TABLE host_snapshot_jobs (
 owner TEXT NOT NULL REFERENCES host_snapshot(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 id TEXT NOT NULL,
 host TEXT NOT NULL,
 pane TEXT NOT NULL,
 state TEXT NOT NULL,
 exit INTEGER CHECK (exit BETWEEN -2147483648 AND 2147483647),
 started_ms INTEGER CHECK (started_ms >= 0),
 ended_ms INTEGER CHECK (ended_ms >= 0),
 job_message BLOB NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_snapshot_jobs_owner ON host_snapshot_jobs(owner,position);"#,
r#"CREATE TABLE host_snapshot (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 sampled_ms INTEGER NOT NULL CHECK (sampled_ms >= 0),
 probe_host TEXT NOT NULL,
 probe_label TEXT NOT NULL,
 probe_load1 REAL NOT NULL,
 probe_cores INTEGER NOT NULL CHECK (probe_cores BETWEEN 0 AND 4294967295),
 probe_cpu_ticks_present INTEGER NOT NULL CHECK(probe_cpu_ticks_present IN (0,1)),
 probe_cpu_ticks_busy INTEGER CHECK (probe_cpu_ticks_busy >= 0),
 probe_cpu_ticks_total INTEGER CHECK (probe_cpu_ticks_total >= 0),
 probe_busy REAL,
 probe_memory_used_bytes INTEGER NOT NULL CHECK (probe_memory_used_bytes >= 0),
 probe_memory_total_bytes INTEGER NOT NULL CHECK (probe_memory_total_bytes >= 0),
 probe_memory_pressure_level INTEGER CHECK (probe_memory_pressure_level BETWEEN 0 AND 255),
 probe_disk_free_percent REAL NOT NULL,
 probe_agents_total INTEGER NOT NULL CHECK (probe_agents_total >= 0)
) STRICT;"#,
r#"CREATE TABLE host_watchdog_sent (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 value INTEGER NOT NULL CHECK (value >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_sent_owner ON host_watchdog_sent(owner,position);"#,
r#"CREATE TABLE host_watchdog_events (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 signature TEXT NOT NULL,
 last_ms INTEGER NOT NULL CHECK (last_ms >= 0),
 urgent INTEGER NOT NULL CHECK (urgent IN (0,1)),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_events_owner ON host_watchdog_events(owner,position);"#,
r#"CREATE TABLE host_watchdog_marks (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_marks_owner ON host_watchdog_marks(owner,position);"#,
r#"CREATE TABLE host_watchdog_notified (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 value INTEGER NOT NULL CHECK (value >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_notified_owner ON host_watchdog_notified(owner,position);"#,
r#"CREATE TABLE host_watchdog_stopped (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 pane TEXT NOT NULL,
 agent_session TEXT NOT NULL,
 turn_state_change_seq INTEGER CHECK (turn_state_change_seq >= 0),
 turn_state_since_ms INTEGER CHECK (turn_state_since_ms >= 0),
 message_id TEXT,
 attempts INTEGER NOT NULL CHECK (attempts BETWEEN 0 AND 255),
 delivered INTEGER NOT NULL CHECK (delivered IN (0,1)),
 prompted_after INTEGER CHECK (prompted_after >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_stopped_owner ON host_watchdog_stopped(owner,position);"#,
r#"CREATE TABLE host_watchdog_prompted_after (
 owner TEXT NOT NULL REFERENCES host_watchdog(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 prompted_after INTEGER NOT NULL CHECK (prompted_after >= 0),
 turn_state_change_seq INTEGER CHECK (turn_state_change_seq >= 0),
 turn_state_since_ms INTEGER CHECK (turn_state_since_ms >= 0),
 working_turn_present INTEGER NOT NULL CHECK(working_turn_present IN (0,1)),
 working_turn_state_change_seq INTEGER CHECK (working_turn_state_change_seq >= 0),
 working_turn_state_since_ms INTEGER CHECK (working_turn_state_since_ms >= 0),
 last_observed_working INTEGER NOT NULL CHECK (last_observed_working IN (0,1)),
 last_observed_ms INTEGER CHECK (last_observed_ms >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_watchdog_prompted_after_owner ON host_watchdog_prompted_after(owner,position);"#,
r#"CREATE TABLE host_watchdog (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 last_scan_ms INTEGER CHECK (last_scan_ms >= 0)
) STRICT;"#,
r#"CREATE TABLE host_peer_source (
 owner TEXT NOT NULL REFERENCES host_peer(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_peer_source_owner ON host_peer_source(owner,position);"#,
r#"CREATE TABLE host_peer (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 fetched_ms INTEGER NOT NULL CHECK (fetched_ms >= 0),
 snapshot_message BLOB,
 reachable INTEGER NOT NULL CHECK (reachable IN (0,1)),
 available INTEGER NOT NULL CHECK (available IN (0,1)),
 running INTEGER CHECK (running IN (0,1)),
 error TEXT,
 failure TEXT CHECK (failure IN ('timeout','connection_refused','host_not_resolved','authentication_failed','remote_command_failed','unexpected_output','deadline','other')),
 retry_attempts INTEGER NOT NULL CHECK (retry_attempts BETWEEN 0 AND 4294967295),
 retry_next_ms INTEGER NOT NULL CHECK (retry_next_ms >= 0),
 job_validity INTEGER CHECK (job_validity BETWEEN 0 AND 4294967295)
) STRICT;"#,
r#"CREATE TABLE host_quota_pools (
 owner TEXT NOT NULL REFERENCES host_quota(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 pool TEXT NOT NULL,
 account TEXT NOT NULL,
 window TEXT NOT NULL,
 used_pct REAL NOT NULL,
 resets_ms INTEGER CHECK (resets_ms >= 0),
 error TEXT,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_quota_pools_owner ON host_quota_pools(owner,position);"#,
r#"CREATE TABLE host_quota (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 version INTEGER NOT NULL CHECK (version BETWEEN 0 AND 4294967295),
 fetched_ms INTEGER NOT NULL CHECK (fetched_ms >= 0),
 source TEXT NOT NULL
) STRICT;"#,
r#"CREATE TABLE host_quota_refresh (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 attempted_ms INTEGER NOT NULL CHECK (attempted_ms >= 0),
 attempts INTEGER NOT NULL CHECK (attempts >= 0),
 retry_attempts INTEGER NOT NULL CHECK (retry_attempts BETWEEN 0 AND 4294967295),
 retry_next_ms INTEGER NOT NULL CHECK (retry_next_ms >= 0),
 pending_push INTEGER NOT NULL CHECK (pending_push IN (0,1)),
 push_retry_attempts INTEGER NOT NULL CHECK (push_retry_attempts BETWEEN 0 AND 4294967295),
 push_retry_next_ms INTEGER NOT NULL CHECK (push_retry_next_ms >= 0)
) STRICT;"#,
r#"CREATE TABLE host_report_applied_results (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_results_owner ON host_report_applied_results(owner,position);"#,
r#"CREATE TABLE host_report_applied_failures (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_failures_owner ON host_report_applied_failures(owner,position);"#,
r#"CREATE TABLE host_report_applied_skipped (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_skipped_owner ON host_report_applied_skipped(owner,position);"#,
r#"CREATE TABLE host_report_applied_failure_entries_entry_dirty_paths (
 owner TEXT NOT NULL REFERENCES host_report_applied_failure_entries(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_failure_entries_entry_dirty_paths_owner ON host_report_applied_failure_entries_entry_dirty_paths(owner,position);"#,
r#"CREATE TABLE host_report_applied_failure_entries (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 entry_class TEXT NOT NULL,
 entry_id TEXT NOT NULL,
 entry_detail TEXT NOT NULL,
 entry_removable INTEGER NOT NULL CHECK (entry_removable IN (0,1)),
 entry_tree_present INTEGER NOT NULL CHECK(entry_tree_present IN (0,1)),
 entry_tree_repo TEXT,
 entry_tree_path TEXT,
 entry_tree_branch TEXT,
 entry_tree_head TEXT,
 entry_tree_identity_present INTEGER CHECK(entry_tree_identity_present IN (0,1)),
 entry_tree_identity_common TEXT,
 entry_tree_identity_gitdir TEXT,
 entry_tree_identity_generation TEXT,
 entry_kind TEXT,
 entry_dirty_count INTEGER CHECK (entry_dirty_count >= 0),
 detail TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_failure_entries_owner ON host_report_applied_failure_entries(owner,position);"#,
r#"CREATE TABLE host_report_applied_repositories_dirty_paths (
 owner TEXT NOT NULL REFERENCES host_report_applied_repositories(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_repositories_dirty_paths_owner ON host_report_applied_repositories_dirty_paths(owner,position);"#,
r#"CREATE TABLE host_report_applied_repositories (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 class TEXT NOT NULL,
 id TEXT NOT NULL,
 detail TEXT NOT NULL,
 removable INTEGER NOT NULL CHECK (removable IN (0,1)),
 tree_present INTEGER NOT NULL CHECK(tree_present IN (0,1)),
 tree_repo TEXT,
 tree_path TEXT,
 tree_branch TEXT,
 tree_head TEXT,
 tree_identity_present INTEGER CHECK(tree_identity_present IN (0,1)),
 tree_identity_common TEXT,
 tree_identity_gitdir TEXT,
 tree_identity_generation TEXT,
 kind TEXT,
 dirty_count INTEGER CHECK (dirty_count >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_repositories_owner ON host_report_applied_repositories(owner,position);"#,
r#"CREATE TABLE host_report_applied_landed_dirty_dirty_paths (
 owner TEXT NOT NULL REFERENCES host_report_applied_landed_dirty(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_landed_dirty_dirty_paths_owner ON host_report_applied_landed_dirty_dirty_paths(owner,position);"#,
r#"CREATE TABLE host_report_applied_landed_dirty (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 class TEXT NOT NULL,
 id TEXT NOT NULL,
 detail TEXT NOT NULL,
 removable INTEGER NOT NULL CHECK (removable IN (0,1)),
 tree_present INTEGER NOT NULL CHECK(tree_present IN (0,1)),
 tree_repo TEXT,
 tree_path TEXT,
 tree_branch TEXT,
 tree_head TEXT,
 tree_identity_present INTEGER CHECK(tree_identity_present IN (0,1)),
 tree_identity_common TEXT,
 tree_identity_gitdir TEXT,
 tree_identity_generation TEXT,
 kind TEXT,
 dirty_count INTEGER CHECK (dirty_count >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_landed_dirty_owner ON host_report_applied_landed_dirty(owner,position);"#,
r#"CREATE TABLE host_report_applied_unlinked_dirty_paths (
 owner TEXT NOT NULL REFERENCES host_report_applied_unlinked(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_unlinked_dirty_paths_owner ON host_report_applied_unlinked_dirty_paths(owner,position);"#,
r#"CREATE TABLE host_report_applied_unlinked (
 owner TEXT NOT NULL REFERENCES host_report(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 class TEXT NOT NULL,
 id TEXT NOT NULL,
 detail TEXT NOT NULL,
 removable INTEGER NOT NULL CHECK (removable IN (0,1)),
 tree_present INTEGER NOT NULL CHECK(tree_present IN (0,1)),
 tree_repo TEXT,
 tree_path TEXT,
 tree_branch TEXT,
 tree_head TEXT,
 tree_identity_present INTEGER CHECK(tree_identity_present IN (0,1)),
 tree_identity_common TEXT,
 tree_identity_gitdir TEXT,
 tree_identity_generation TEXT,
 kind TEXT,
 dirty_count INTEGER CHECK (dirty_count >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_report_applied_unlinked_owner ON host_report_applied_unlinked(owner,position);"#,
r#"CREATE TABLE host_report (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0),
 ts_ms INTEGER NOT NULL CHECK (ts_ms >= 0),
 started_ms INTEGER NOT NULL CHECK (started_ms >= 0),
 finished_ms INTEGER NOT NULL CHECK (finished_ms >= 0)
) STRICT;
CREATE INDEX host_report_ts ON host_report(ts_ms);"#,
r#"CREATE TABLE host_marks_entries (
 owner TEXT NOT NULL REFERENCES host_marks(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 pane TEXT NOT NULL,
 item TEXT,
 session TEXT,
 name TEXT,
 after INTEGER CHECK (after >= 0),
 sealed INTEGER CHECK (sealed >= 0),
 marked_ms INTEGER NOT NULL CHECK (marked_ms >= 0),
 done_turn_present INTEGER NOT NULL CHECK(done_turn_present IN (0,1)),
 done_turn_state_change_seq INTEGER CHECK (done_turn_state_change_seq >= 0),
 done_turn_state_since_ms INTEGER CHECK (done_turn_state_since_ms >= 0),
 signal_present INTEGER NOT NULL CHECK(signal_present IN (0,1)),
 signal_kind TEXT CHECK (signal_kind IN ('ask','blocked','waiting')),
 signal_text TEXT,
 signal_event_id TEXT,
 signal_turn_state_change_seq INTEGER CHECK (signal_turn_state_change_seq >= 0),
 signal_turn_state_since_ms INTEGER CHECK (signal_turn_state_since_ms >= 0),
 signal_session TEXT,
 signal_recorded_ms INTEGER CHECK (signal_recorded_ms >= 0),
 signal_cleared_ms INTEGER CHECK (signal_cleared_ms >= 0),
 signal_history_message BLOB,
 signal_inputs_message BLOB,
 signal_delivered INTEGER NOT NULL CHECK(signal_delivered IN (0,1)),
 branch_present INTEGER NOT NULL CHECK(branch_present IN (0,1)),
 branch_repo TEXT,
 branch_name TEXT,
 branch_ahead INTEGER CHECK (branch_ahead >= 0),
 branch_landed INTEGER CHECK (branch_landed IN (0,1)),
 detached INTEGER NOT NULL CHECK (detached IN (0,1)),
 reply_present INTEGER NOT NULL CHECK(reply_present IN (0,1)),
 reply_seq INTEGER CHECK (reply_seq >= 0),
 reply_result_id TEXT,
 reply_text TEXT,
 reply_captured_ms INTEGER CHECK (reply_captured_ms >= 0),
 reply_reported_ms INTEGER CHECK (reply_reported_ms >= 0),
 reply_done INTEGER CHECK (reply_done IN (0,1)),
 native_done INTEGER NOT NULL CHECK (native_done IN (0,1)),
 closed_ms INTEGER CHECK (closed_ms >= 0),
 dropped_ms INTEGER CHECK (dropped_ms >= 0),
 PRIMARY KEY(owner,position),
 UNIQUE(owner,pane,item,session)
) STRICT;
CREATE INDEX host_marks_entries_owner ON host_marks_entries(owner,position);
CREATE UNIQUE INDEX host_marks_identity ON host_marks_entries(owner,pane,ifnull(item,''),ifnull(session,''));"#,
r#"CREATE TABLE host_marks (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_trees_entries (
 owner TEXT NOT NULL REFERENCES host_trees(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 repo TEXT NOT NULL,
 path TEXT NOT NULL,
 branch TEXT NOT NULL,
 head TEXT NOT NULL,
 identity_present INTEGER NOT NULL CHECK(identity_present IN (0,1)),
 identity_common TEXT,
 identity_gitdir TEXT,
 identity_generation TEXT,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_trees_entries_owner ON host_trees_entries(owner,position);"#,
r#"CREATE TABLE host_trees (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_repositories_entries (
 owner TEXT NOT NULL REFERENCES host_repositories(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_repositories_entries_owner ON host_repositories_entries(owner,position);"#,
r#"CREATE TABLE host_repositories (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_custodies_entries_claims (
 owner TEXT NOT NULL REFERENCES host_custodies_entries(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 id TEXT NOT NULL,
 claimant_pane TEXT NOT NULL,
 claimant_session TEXT,
 claimed_ms INTEGER NOT NULL CHECK (claimed_ms >= 0),
 previous_custodian TEXT,
 revision INTEGER NOT NULL CHECK (revision >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_custodies_entries_claims_owner ON host_custodies_entries_claims(owner,position);"#,
r#"CREATE TABLE host_custodies_entries (
 owner TEXT NOT NULL REFERENCES host_custodies(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 item TEXT NOT NULL,
 host TEXT NOT NULL,
 owner_pane TEXT NOT NULL,
 owner_session TEXT,
 custodian_pane TEXT NOT NULL,
 custodian_session TEXT,
 revision INTEGER NOT NULL CHECK (revision >= 0),
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_custodies_entries_owner ON host_custodies_entries(owner,position);"#,
r#"CREATE TABLE host_custodies (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_parents_entries (
 owner TEXT NOT NULL REFERENCES host_parents(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 transcript TEXT,
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_parents_entries_owner ON host_parents_entries(owner,position);"#,
r#"CREATE TABLE host_parents (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_landing_entries (
 owner TEXT NOT NULL REFERENCES host_landing(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 value TEXT NOT NULL,
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_landing_entries_owner ON host_landing_entries(owner,position);"#,
r#"CREATE TABLE host_landing (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_orphans_entries (
 owner TEXT NOT NULL REFERENCES host_orphans(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 item TEXT NOT NULL,
 host TEXT NOT NULL,
 pane TEXT NOT NULL,
 owner_pane TEXT NOT NULL,
 name TEXT,
 reason TEXT NOT NULL,
 since_ms INTEGER NOT NULL CHECK (since_ms >= 0),
 status TEXT NOT NULL CHECK (status IN ('orphaned','unknown')),
 claimed INTEGER NOT NULL CHECK (claimed IN (0,1)),
 custodian_pane TEXT,
 PRIMARY KEY(owner,position)
) STRICT;
CREATE INDEX host_orphans_entries_owner ON host_orphans_entries(owner,position);"#,
r#"CREATE TABLE host_orphans (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;"#,
r#"CREATE TABLE host_operations_entries (
 owner TEXT NOT NULL REFERENCES host_operations(row_key) ON DELETE CASCADE,
 position INTEGER NOT NULL CHECK(position >= 0),
 row_key TEXT NOT NULL UNIQUE,
 map_key TEXT NOT NULL,
 id TEXT NOT NULL,
 expectation_pane TEXT NOT NULL,
 expectation_terminal TEXT,
 expectation_request_message BLOB,
 expectation_launch TEXT,
 expectation_owners_message BLOB NOT NULL,
 expectation_tree_repo TEXT NOT NULL,
 expectation_tree_path TEXT NOT NULL,
 expectation_tree_branch TEXT NOT NULL,
 expectation_tree_head TEXT NOT NULL,
 expectation_tree_identity_present INTEGER NOT NULL CHECK(expectation_tree_identity_present IN (0,1)),
 expectation_tree_identity_common TEXT,
 expectation_tree_identity_gitdir TEXT,
 expectation_tree_identity_generation TEXT,
 expectation_mode TEXT NOT NULL CHECK(expectation_mode IN ('owner','stopped_orphan')),
 expectation_mode_episode_message BLOB,
 expectation_caller_pane TEXT,
 expectation_caller_workspace TEXT,
 expectation_caller_cwd TEXT,
 initial_id TEXT NOT NULL,
 successor_id TEXT,
 initial_message BLOB,
 latest_message BLOB,
 stage TEXT NOT NULL CHECK (stage IN ('intent','captured','teardown_entered','teardown_complete','final_captured','normalizing','normalized','removed','finished')),
 blocker TEXT,
 result_message BLOB,
 PRIMARY KEY(owner,position),
 UNIQUE(owner,map_key)
) STRICT;
CREATE INDEX host_operations_entries_owner ON host_operations_entries(owner,position);"#,
r#"CREATE TABLE host_operations (
 row_key TEXT PRIMARY KEY CHECK(length(row_key) > 0)
) STRICT;
CREATE INDEX host_tree_path ON host_trees_entries(repo,path);
CREATE INDEX host_repository_path ON host_repositories_entries(owner,value);
CREATE INDEX host_custody_item ON host_custodies_entries(owner,item);
CREATE INDEX host_orphan_item ON host_orphans_entries(owner,item);
CREATE INDEX host_operation_tree ON host_operations_entries(expectation_tree_path,stage);
CREATE INDEX host_operation_pane ON host_operations_entries(expectation_pane,expectation_launch);
CREATE INDEX host_snapshot_job_state ON host_snapshot_jobs(owner,state,id);
CREATE INDEX host_snapshot_agent_pane ON host_snapshot_probe_agent_panes(owner,pane);
CREATE INDEX host_snapshot_pane ON host_snapshot_probe_panes(owner,pane_id);
CREATE INDEX host_quota_account ON host_quota_pools(owner,pool,account,window);"#
);
struct Marks {
    entries: BTreeMap<String, crate::done::Mark>,
}
struct Trees {
    entries: Vec<crate::cleanup::Tree>,
}
struct Repositories {
    entries: BTreeSet<String>,
}
struct Custodies {
    entries: Vec<crate::custody::Custody>,
}
struct Parents {
    entries: BTreeMap<String, crate::watchdog::Parent>,
}
struct Landing {
    entries: BTreeMap<String, String>,
}
struct Orphans {
    entries: Vec<crate::sweep::Orphan>,
}
struct Operations {
    entries: BTreeMap<String, crate::preservation::Operation>,
}
fn put_snapshot_probe_agents_by_status(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: usize,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.number(
        "value",
        Some(u64::try_from(value).map_err(|_| invalid("integer out of range"))?),
    )?;
    columns.upsert(tx, "host_snapshot_probe_agents_by_status", "row_key")?;
    Ok(())
}

fn load_snapshot_probe_agents_by_status(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, usize)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_snapshot_probe_agents_by_status WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            number::<usize>(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_snapshot_probe_agent_panes(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::probe::Agent,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("pane", Some(&value.pane));
    if let Some(value) = &value.agent {
        columns.text("agent", Some(value));
    }
    columns.text("state", Some(&value.state));
    if let Some(value) = &value.name {
        columns.text("name", Some(value));
    }
    if let Some(value) = &value.transcript {
        columns.text("transcript", Some(value));
    }
    if let Some(value) = &value.transcript_modified {
        columns.number("transcript_modified", Some(*value))?;
    }
    if let Some(value) = &value.stalled_since_ms {
        columns.number("stalled_since_ms", Some(*value))?;
    }
    if let Some(value) = &value.state_since_ms {
        columns.number("state_since_ms", Some(*value))?;
    }
    if let Some(value) = &value.signal {
        columns.text("signal", Some(value));
    }
    if let Some(value) = &value.terminal_id {
        columns.text("terminal_id", Some(value));
    }
    if let Some(value) = &value.agent_session {
        columns.text("agent_session", Some(value));
    }
    if let Some(value) = &value.completion_seq {
        columns.number("completion_seq", Some(*value))?;
    }
    if let Some(value) = &value.state_change_seq {
        columns.number("state_change_seq", Some(*value))?;
    }
    if let Some(value) = &value.cwd {
        columns.text("cwd", Some(value));
    }
    if let Some(value) = &value.session.model {
        columns.text("session_model", Some(value));
    }
    if let Some(value) = &value.session.response_model {
        columns.text("session_response_model", Some(value));
    }
    if let Some(value) = &value.session.thinking {
        columns.text("session_thinking", Some(value));
    }
    if let Some(value) = &value.session.activity {
        columns.text("session_activity", Some(value));
    }
    columns.flag("worktree_present", value.worktree.is_some());
    if let Some(value) = &value.worktree {
        columns.text("worktree_path", Some(&value.path));
        columns.text("worktree_branch", Some(&value.branch));
        columns.number("worktree_ahead", Some(value.ahead))?;
        columns.flag("worktree_dirty", value.dirty);
        columns.flag("worktree_landed", value.landed);
        columns.text("worktree_head", Some(&value.head));
        columns.text("worktree_base", Some(&value.base));
    }
    if let Some(value) = &value.repository {
        columns.text("repository", Some(value));
    }
    columns.flag("done", value.done);
    columns.flag("finishing", value.finishing);
    if let Some(value) = &value.handoff {
        columns.text("handoff", Some(value));
    }
    if let Some(value) = &value.launch_progress {
        columns.text("launch_progress", Some(value));
    }
    columns.flag("owns_open", value.owns_open);
    if let Some(value) = &value.lifecycle {
        columns.text("lifecycle", Some(value));
    }
    if let Some(value) = &value.lifecycle_gap {
        columns.text("lifecycle_gap", Some(value));
    }
    columns.upsert(tx, "host_snapshot_probe_agent_panes", "row_key")?;
    Ok(())
}

fn load_snapshot_probe_agent_panes(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::probe::Agent)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_snapshot_probe_agent_panes WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::probe::Agent {
                pane: rows::required(row, "pane")?,
                agent: rows::text(row, "agent")?,
                state: rows::required(row, "state")?,
                name: rows::text(row, "name")?,
                transcript: rows::text(row, "transcript")?,
                transcript_modified: optional_number::<u64>(row, "transcript_modified")?,
                stalled_since_ms: optional_number::<u64>(row, "stalled_since_ms")?,
                state_since_ms: optional_number::<u64>(row, "state_since_ms")?,
                signal: rows::text(row, "signal")?,
                terminal_id: rows::text(row, "terminal_id")?,
                agent_session: rows::text(row, "agent_session")?,
                completion_seq: optional_number::<u64>(row, "completion_seq")?,
                state_change_seq: optional_number::<u64>(row, "state_change_seq")?,
                cwd: rows::text(row, "cwd")?,
                session: crate::facts::Session {
                    model: rows::text(row, "session_model")?,
                    response_model: rows::text(row, "session_response_model")?,
                    thinking: rows::text(row, "session_thinking")?,
                    activity: rows::text(row, "session_activity")?,
                },
                worktree: if row.get::<_, Option<bool>>("worktree_present")? == Some(true) {
                    Some(crate::facts::Worktree {
                        path: rows::required(row, "worktree_path")?,
                        branch: rows::required(row, "worktree_branch")?,
                        ahead: number::<u64>(row, "worktree_ahead")?,
                        dirty: row.get("worktree_dirty")?,
                        landed: row.get("worktree_landed")?,
                        head: rows::required(row, "worktree_head")?,
                        base: rows::required(row, "worktree_base")?,
                    })
                } else {
                    None
                },
                repository: rows::text(row, "repository")?,
                done: row.get("done")?,
                finishing: row.get("finishing")?,
                handoff: rows::text(row, "handoff")?,
                launch_progress: rows::text(row, "launch_progress")?,
                owns_open: row.get("owns_open")?,
                lifecycle: rows::text(row, "lifecycle")?,
                lifecycle_gap: rows::text(row, "lifecycle_gap")?,
            },
        ));
    }
    Ok(values)
}
fn put_snapshot_probe_reservations(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::ledger::Reservation,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("id", Some(&value.id));
    columns.number("created_ms", Some(value.created_ms))?;
    columns.upsert(tx, "host_snapshot_probe_reservations", "row_key")?;
    Ok(())
}

fn load_snapshot_probe_reservations(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::ledger::Reservation)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_snapshot_probe_reservations WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::ledger::Reservation {
                id: rows::required(row, "id")?,
                created_ms: number::<u64>(row, "created_ms")?,
            },
        ));
    }
    Ok(values)
}
fn put_snapshot_probe_panes(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::probe::Pane,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("pane_id", Some(&value.pane_id));
    columns.flag("focused", value.focused);
    if let Some(value) = &value.workspace_id {
        columns.text("workspace_id", Some(value));
    }
    if let Some(value) = &value.name {
        columns.text("name", Some(value));
    }
    if let Some(value) = &value.agent {
        columns.text("agent", Some(value));
    }
    if let Some(value) = &value.agent_status {
        columns.text("agent_status", Some(value));
    }
    if let Some(value) = &value.cwd {
        columns.text("cwd", Some(value));
    }
    if let Some(value) = &value.foreground_cwd {
        columns.text("foreground_cwd", Some(value));
    }
    columns.flag("idle_shell", value.idle_shell);
    if let Some(value) = &value.idle_since_ms {
        columns.number("idle_since_ms", Some(*value))?;
    }
    columns.upsert(tx, "host_snapshot_probe_panes", "row_key")?;
    Ok(())
}

fn load_snapshot_probe_panes(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::probe::Pane)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_snapshot_probe_panes WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::probe::Pane {
                pane_id: rows::required(row, "pane_id")?,
                focused: row.get("focused")?,
                workspace_id: rows::text(row, "workspace_id")?,
                name: rows::text(row, "name")?,
                agent: rows::text(row, "agent")?,
                agent_status: rows::text(row, "agent_status")?,
                cwd: rows::text(row, "cwd")?,
                foreground_cwd: rows::text(row, "foreground_cwd")?,
                idle_shell: row.get("idle_shell")?,
                idle_since_ms: optional_number::<u64>(row, "idle_since_ms")?,
            },
        ));
    }
    Ok(values)
}
fn put_snapshot_jobs(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::jobs::Job,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("id", Some(&value.id));
    columns.text("host", Some(&value.host));
    columns.text("pane", Some(&value.pane));
    columns.text("state", Some(&value.state));
    if let Some(value) = &value.exit {
        columns.put("exit", Sql::Integer(i64::from(*value)));
    }
    if let Some(value) = &value.started_ms {
        columns.number("started_ms", Some(*value))?;
    }
    if let Some(value) = &value.ended_ms {
        columns.number("ended_ms", Some(*value))?;
    }
    columns.message("job_message", Some(value))?;
    columns.upsert(tx, "host_snapshot_jobs", "row_key")?;
    Ok(())
}

fn load_snapshot_jobs(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::jobs::Job)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_snapshot_jobs WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        let mut job: crate::jobs::Job = rows::decoded(row, "job_message")?
            .ok_or_else(|| invalid("missing snapshot job message"))?;
        job.id = rows::required(row, "id")?;
        job.host = rows::required(row, "host")?;
        job.pane = rows::required(row, "pane")?;
        job.state = rows::required(row, "state")?;
        job.exit = row.get("exit")?;
        job.started_ms = optional_number(row, "started_ms")?;
        job.ended_ms = optional_number(row, "ended_ms")?;
        values.push((row.get("position")?, String::new(), job));
    }
    Ok(values)
}
fn snapshot_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::node::Snapshot>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_snapshot WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::node::Snapshot {
        sampled_ms: number::<u64>(row, "sampled_ms")?,
        probe: crate::probe::Probe {
            host: rows::required(row, "probe_host")?,
            label: rows::required(row, "probe_label")?,
            load1: row.get("probe_load1")?,
            cores: number::<u32>(row, "probe_cores")?,
            cpu_ticks: if row.get::<_, Option<bool>>("probe_cpu_ticks_present")? == Some(true) {
                Some(crate::probe::CpuTicks {
                    busy: number::<u64>(row, "probe_cpu_ticks_busy")?,
                    total: number::<u64>(row, "probe_cpu_ticks_total")?,
                })
            } else {
                None
            },
            busy: row.get("probe_busy")?,
            memory: crate::probe::Memory {
                used_bytes: number::<u64>(row, "probe_memory_used_bytes")?,
                total_bytes: number::<u64>(row, "probe_memory_total_bytes")?,
                pressure_level: optional_number::<u8>(row, "probe_memory_pressure_level")?,
            },
            disk_free_percent: row.get("probe_disk_free_percent")?,
            agents: crate::probe::AgentCounts {
                total: number::<usize>(row, "probe_agents_total")?,
                by_status: load_snapshot_probe_agents_by_status(tx, id)?
                    .into_iter()
                    .map(|(_, key, value)| (key, value))
                    .collect(),
            },
            agent_panes: load_snapshot_probe_agent_panes(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            reservations: load_snapshot_probe_reservations(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            panes: load_snapshot_probe_panes(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
        },
        jobs: load_snapshot_jobs(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_snapshot_row(tx: &Transaction<'_>, id: &str, value: &crate::node::Snapshot) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.number("sampled_ms", Some(value.sampled_ms))?;
    columns.text("probe_host", Some(&value.probe.host));
    columns.text("probe_label", Some(&value.probe.label));
    columns.put("probe_load1", Sql::Real(finite(value.probe.load1)?));
    columns.number("probe_cores", Some(u64::from(value.probe.cores)))?;
    columns.flag("probe_cpu_ticks_present", value.probe.cpu_ticks.is_some());
    if let Some(value) = &value.probe.cpu_ticks {
        columns.number("probe_cpu_ticks_busy", Some(value.busy))?;
        columns.number("probe_cpu_ticks_total", Some(value.total))?;
    }
    if let Some(value) = &value.probe.busy {
        columns.put("probe_busy", Sql::Real(finite(*value)?));
    }
    columns.number(
        "probe_memory_used_bytes",
        Some(value.probe.memory.used_bytes),
    )?;
    columns.number(
        "probe_memory_total_bytes",
        Some(value.probe.memory.total_bytes),
    )?;
    if let Some(value) = &value.probe.memory.pressure_level {
        columns.number("probe_memory_pressure_level", Some(u64::from(*value)))?;
    }
    columns.put(
        "probe_disk_free_percent",
        Sql::Real(finite(value.probe.disk_free_percent)?),
    );
    columns.number(
        "probe_agents_total",
        Some(u64::try_from(value.probe.agents.total).map_err(|_| invalid("integer out of range"))?),
    )?;
    columns.upsert(tx, "host_snapshot", "row_key")?;
    tx.execute(
        "DELETE FROM host_snapshot_probe_agents_by_status WHERE owner = ?1",
        [id],
    )?;
    for (position, (key, child)) in (0_i64..).zip(value.probe.agents.by_status.iter()) {
        put_snapshot_probe_agents_by_status(tx, id, position, key, *child)?;
    }
    tx.execute(
        "DELETE FROM host_snapshot_probe_agent_panes WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.probe.agent_panes.iter()) {
        put_snapshot_probe_agent_panes(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_snapshot_probe_reservations WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.probe.reservations.iter()) {
        put_snapshot_probe_reservations(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_snapshot_probe_panes WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.probe.panes.iter()) {
        put_snapshot_probe_panes(tx, id, position, "", child)?;
    }
    tx.execute("DELETE FROM host_snapshot_jobs WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.jobs.iter()) {
        put_snapshot_jobs(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_watchdog_sent(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: u64,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.number("value", Some(value))?;
    columns.upsert(tx, "host_watchdog_sent", "row_key")?;
    Ok(())
}

fn load_watchdog_sent(tx: &Transaction<'_>, owner: &str) -> Result<Vec<(i64, String, u64)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_watchdog_sent WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            number::<u64>(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_watchdog_events(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::watchdog::Event,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.text("signature", Some(&value.signature));
    columns.number("last_ms", Some(value.last_ms))?;
    columns.flag("urgent", value.urgent);
    columns.upsert(tx, "host_watchdog_events", "row_key")?;
    Ok(())
}

fn load_watchdog_events(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::watchdog::Event)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_watchdog_events WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            crate::watchdog::Event {
                signature: rows::required(row, "signature")?,
                last_ms: number::<u64>(row, "last_ms")?,
                urgent: row.get("urgent")?,
            },
        ));
    }
    Ok(values)
}
fn put_watchdog_marks(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_watchdog_marks", "row_key")?;
    Ok(())
}

fn load_watchdog_marks(tx: &Transaction<'_>, owner: &str) -> Result<Vec<(i64, String, String)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_watchdog_marks WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_watchdog_notified(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: u64,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.number("value", Some(value))?;
    columns.upsert(tx, "host_watchdog_notified", "row_key")?;
    Ok(())
}

fn load_watchdog_notified(tx: &Transaction<'_>, owner: &str) -> Result<Vec<(i64, String, u64)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_watchdog_notified WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            number::<u64>(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_watchdog_stopped(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::watchdog::StoppedEpisode,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.text("pane", Some(&value.pane));
    columns.text("agent_session", Some(&value.agent_session));
    if let Some(value) = &value.turn.state_change_seq {
        columns.number("turn_state_change_seq", Some(*value))?;
    }
    if let Some(value) = &value.turn.state_since_ms {
        columns.number("turn_state_since_ms", Some(*value))?;
    }
    if let Some(value) = &value.message_id {
        columns.text("message_id", Some(value));
    }
    columns.number("attempts", Some(u64::from(value.attempts)))?;
    columns.flag("delivered", value.delivered);
    if let Some(value) = &value.prompted_after {
        columns.number("prompted_after", Some(*value))?;
    }
    columns.upsert(tx, "host_watchdog_stopped", "row_key")?;
    Ok(())
}

fn load_watchdog_stopped(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::watchdog::StoppedEpisode)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_watchdog_stopped WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            crate::watchdog::StoppedEpisode {
                pane: rows::required(row, "pane")?,
                agent_session: rows::required(row, "agent_session")?,
                turn: crate::done::Turn {
                    state_change_seq: optional_number::<u64>(row, "turn_state_change_seq")?,
                    state_since_ms: optional_number::<u64>(row, "turn_state_since_ms")?,
                },
                message_id: rows::text(row, "message_id")?,
                attempts: number::<u8>(row, "attempts")?,
                delivered: row.get("delivered")?,
                prompted_after: optional_number::<u64>(row, "prompted_after")?,
            },
        ));
    }
    Ok(values)
}
fn put_watchdog_prompted_after(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::watchdog::PromptHistory,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.number("prompted_after", Some(value.prompted_after))?;
    if let Some(value) = &value.turn.state_change_seq {
        columns.number("turn_state_change_seq", Some(*value))?;
    }
    if let Some(value) = &value.turn.state_since_ms {
        columns.number("turn_state_since_ms", Some(*value))?;
    }
    columns.flag("working_turn_present", value.working_turn.is_some());
    if let Some(value) = &value.working_turn {
        if let Some(value) = &value.state_change_seq {
            columns.number("working_turn_state_change_seq", Some(*value))?;
        }
        if let Some(value) = &value.state_since_ms {
            columns.number("working_turn_state_since_ms", Some(*value))?;
        }
    }
    columns.flag("last_observed_working", value.last_observed_working);
    if let Some(value) = &value.last_observed_ms {
        columns.number("last_observed_ms", Some(*value))?;
    }
    columns.upsert(tx, "host_watchdog_prompted_after", "row_key")?;
    Ok(())
}

fn load_watchdog_prompted_after(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::watchdog::PromptHistory)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_watchdog_prompted_after WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            crate::watchdog::PromptHistory {
                prompted_after: number::<u64>(row, "prompted_after")?,
                turn: crate::done::Turn {
                    state_change_seq: optional_number::<u64>(row, "turn_state_change_seq")?,
                    state_since_ms: optional_number::<u64>(row, "turn_state_since_ms")?,
                },
                working_turn: if row.get::<_, Option<bool>>("working_turn_present")? == Some(true) {
                    Some(crate::done::Turn {
                        state_change_seq: optional_number::<u64>(
                            row,
                            "working_turn_state_change_seq",
                        )?,
                        state_since_ms: optional_number::<u64>(row, "working_turn_state_since_ms")?,
                    })
                } else {
                    None
                },
                last_observed_working: row.get("last_observed_working")?,
                last_observed_ms: optional_number::<u64>(row, "last_observed_ms")?,
            },
        ));
    }
    Ok(values)
}
fn watchdog_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::watchdog::State>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_watchdog WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::watchdog::State {
        last_scan_ms: optional_number::<u64>(row, "last_scan_ms")?,
        sent: load_watchdog_sent(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
        events: load_watchdog_events(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
        marks: load_watchdog_marks(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
        notified: load_watchdog_notified(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
        stopped: load_watchdog_stopped(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
        prompted_after: load_watchdog_prompted_after(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    }))
}
fn put_watchdog_row(tx: &Transaction<'_>, id: &str, value: &crate::watchdog::State) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    if let Some(value) = &value.last_scan_ms {
        columns.number("last_scan_ms", Some(*value))?;
    }
    columns.upsert(tx, "host_watchdog", "row_key")?;
    tx.execute("DELETE FROM host_watchdog_sent WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.sent.iter()) {
        put_watchdog_sent(tx, id, position, key, *child)?;
    }
    tx.execute("DELETE FROM host_watchdog_events WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.events.iter()) {
        put_watchdog_events(tx, id, position, key, child)?;
    }
    tx.execute("DELETE FROM host_watchdog_marks WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.marks.iter()) {
        put_watchdog_marks(tx, id, position, key, child)?;
    }
    tx.execute("DELETE FROM host_watchdog_notified WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.notified.iter()) {
        put_watchdog_notified(tx, id, position, key, *child)?;
    }
    tx.execute("DELETE FROM host_watchdog_stopped WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.stopped.iter()) {
        put_watchdog_stopped(tx, id, position, key, child)?;
    }
    tx.execute(
        "DELETE FROM host_watchdog_prompted_after WHERE owner = ?1",
        [id],
    )?;
    for (position, (key, child)) in (0_i64..).zip(value.prompted_after.iter()) {
        put_watchdog_prompted_after(tx, id, position, key, child)?;
    }
    Ok(())
}

fn put_peer_source(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_peer_source", "row_key")?;
    Ok(())
}

fn load_peer_source(tx: &Transaction<'_>, owner: &str) -> Result<Vec<(i64, String, String)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_peer_source WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
pub(crate) fn peer_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::hosts::Cached>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_peer WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::hosts::Cached {
        fetched_ms: number::<u64>(row, "fetched_ms")?,
        source: load_peer_source(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
        snapshot: rows::decoded(row, "snapshot_message")?,
        reachable: row.get("reachable")?,
        available: row.get("available")?,
        running: row.get("running")?,
        handoffs: peer_rows::handoffs(tx, id)?
            .into_iter()
            .map(|(_, _, mark)| mark)
            .collect(),
        holds: peer_rows::holds(tx, id)?,
        error: rows::text(row, "error")?,
        failure: rows::optional(rows::text(row, "failure")?, rows::variant)?,
        retry: crate::backoff::Backoff {
            attempts: number::<u32>(row, "retry_attempts")?,
            next_ms: number::<u64>(row, "retry_next_ms")?,
        },
        service_lanes: peer_rows::service_lanes(tx, id)?,
        job_validity: optional_number::<u32>(row, "job_validity")?,
    }))
}
pub(crate) fn put_peer_row(
    tx: &Transaction<'_>,
    id: &str,
    value: &crate::hosts::Cached,
) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.number("fetched_ms", Some(value.fetched_ms))?;
    columns.message("snapshot_message", value.snapshot.as_ref())?;
    columns.flag("reachable", value.reachable);
    columns.flag("available", value.available);
    if let Some(value) = &value.running {
        columns.flag("running", *value);
    }
    if let Some(value) = &value.error {
        columns.text("error", Some(value));
    }
    if let Some(value) = &value.failure {
        columns.text("failure", Some(&rows::name(value)?));
    }
    columns.number("retry_attempts", Some(u64::from(value.retry.attempts)))?;
    columns.number("retry_next_ms", Some(value.retry.next_ms))?;
    if let Some(value) = &value.job_validity {
        columns.number("job_validity", Some(u64::from(*value)))?;
    }
    columns.upsert(tx, "host_peer", "row_key")?;
    tx.execute("DELETE FROM host_peer_source WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.source.iter()) {
        put_peer_source(tx, id, position, "", child)?;
    }
    tx.execute("DELETE FROM host_peer_handoffs WHERE owner = ?1", [id])?;
    tx.execute("DELETE FROM host_peer_holds WHERE owner = ?1", [id])?;
    for (position, mark) in (0_i64..).zip(&value.handoffs) {
        peer_rows::put_handoff(tx, id, position, "", mark)?;
    }
    for (position, hold) in (0_i64..).zip(&value.holds) {
        peer_rows::put_hold(tx, id, position, hold)?;
    }
    peer_rows::put_service_lanes(tx, id, &value.service_lanes)?;
    Ok(())
}

fn put_quota_pools(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::quota::Row,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("pool", Some(&value.pool));
    columns.text("account", Some(&value.account));
    columns.text("window", Some(&value.window));
    columns.put("used_pct", Sql::Real(finite(value.used_pct)?));
    if let Some(value) = &value.resets_ms {
        columns.number("resets_ms", Some(*value))?;
    }
    if let Some(value) = &value.error {
        columns.text("error", Some(value));
    }
    columns.upsert(tx, "host_quota_pools", "row_key")?;
    Ok(())
}

fn load_quota_pools(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::quota::Row)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_quota_pools WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::quota::Row {
                pool: rows::required(row, "pool")?,
                account: rows::required(row, "account")?,
                window: rows::required(row, "window")?,
                used_pct: row.get("used_pct")?,
                resets_ms: optional_number::<u64>(row, "resets_ms")?,
                error: rows::text(row, "error")?,
            },
        ));
    }
    Ok(values)
}
fn quota_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::quota::Snapshot>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_quota WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::quota::Snapshot {
        version: number::<u32>(row, "version")?,
        fetched_ms: number::<u64>(row, "fetched_ms")?,
        source: rows::required(row, "source")?,
        pools: load_quota_pools(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_quota_row(tx: &Transaction<'_>, id: &str, value: &crate::quota::Snapshot) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.number("version", Some(u64::from(value.version)))?;
    columns.number("fetched_ms", Some(value.fetched_ms))?;
    columns.text("source", Some(&value.source));
    columns.upsert(tx, "host_quota", "row_key")?;
    tx.execute("DELETE FROM host_quota_pools WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.pools.iter()) {
        put_quota_pools(tx, id, position, "", child)?;
    }
    Ok(())
}

fn quota_refresh_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::quota::Refresh>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_quota_refresh WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::quota::Refresh {
        attempted_ms: number::<u64>(row, "attempted_ms")?,
        attempts: number::<u64>(row, "attempts")?,
        retry: crate::backoff::Backoff {
            attempts: number::<u32>(row, "retry_attempts")?,
            next_ms: number::<u64>(row, "retry_next_ms")?,
        },
        pending_push: row.get("pending_push")?,
        push_retry: crate::backoff::Backoff {
            attempts: number::<u32>(row, "push_retry_attempts")?,
            next_ms: number::<u64>(row, "push_retry_next_ms")?,
        },
    }))
}
fn put_quota_refresh_row(
    tx: &Transaction<'_>,
    id: &str,
    value: &crate::quota::Refresh,
) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.number("attempted_ms", Some(value.attempted_ms))?;
    columns.number("attempts", Some(value.attempts))?;
    columns.number("retry_attempts", Some(u64::from(value.retry.attempts)))?;
    columns.number("retry_next_ms", Some(value.retry.next_ms))?;
    columns.flag("pending_push", value.pending_push);
    columns.number(
        "push_retry_attempts",
        Some(u64::from(value.push_retry.attempts)),
    )?;
    columns.number("push_retry_next_ms", Some(value.push_retry.next_ms))?;
    columns.upsert(tx, "host_quota_refresh", "row_key")?;
    Ok(())
}

fn put_report_applied_results(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_report_applied_results", "row_key")?;
    Ok(())
}

fn load_report_applied_results(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_results WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_failures(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_report_applied_failures", "row_key")?;
    Ok(())
}

fn load_report_applied_failures(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_failures WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_skipped(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_report_applied_skipped", "row_key")?;
    Ok(())
}

fn load_report_applied_skipped(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_skipped WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_failure_entries_entry_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(
        tx,
        "host_report_applied_failure_entries_entry_dirty_paths",
        "row_key",
    )?;
    Ok(())
}

fn load_report_applied_failure_entries_entry_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement=tx.prepare_cached("SELECT * FROM host_report_applied_failure_entries_entry_dirty_paths WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_failure_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::cleanup::Failure,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("entry_class", Some(&value.entry.class));
    columns.text("entry_id", Some(&value.entry.id));
    columns.text("entry_detail", Some(&value.entry.detail));
    columns.flag("entry_removable", value.entry.removable);
    columns.flag("entry_tree_present", value.entry.tree.is_some());
    if let Some(value) = &value.entry.tree {
        columns.text("entry_tree_repo", Some(&value.repo));
        columns.text("entry_tree_path", Some(&value.path));
        columns.text("entry_tree_branch", Some(&value.branch));
        columns.text("entry_tree_head", Some(&value.head));
        columns.flag("entry_tree_identity_present", value.identity.is_some());
        if let Some(value) = &value.identity {
            columns.text(
                "entry_tree_identity_common",
                Some(&path_text(&value.common)?),
            );
            columns.text(
                "entry_tree_identity_gitdir",
                Some(&path_text(&value.gitdir)?),
            );
            columns.text("entry_tree_identity_generation", Some(&value.generation));
        }
    }
    if let Some(value) = &value.entry.kind {
        columns.text("entry_kind", Some(value));
    }
    if let Some(value) = &value.entry.dirty_count {
        columns.number(
            "entry_dirty_count",
            Some(u64::try_from(*value).map_err(|_| invalid("integer out of range"))?),
        )?;
    }
    columns.text("detail", Some(&value.detail));
    columns.upsert(tx, "host_report_applied_failure_entries", "row_key")?;
    tx.execute(
        "DELETE FROM host_report_applied_failure_entries_entry_dirty_paths WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.entry.dirty_paths.iter()) {
        put_report_applied_failure_entries_entry_dirty_paths(tx, id, position, "", child)?;
    }
    Ok(())
}

fn load_report_applied_failure_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::cleanup::Failure)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_failure_entries WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::cleanup::Failure {
                entry: crate::cleanup::Entry {
                    class: rows::required(row, "entry_class")?,
                    id: rows::required(row, "entry_id")?,
                    detail: rows::required(row, "entry_detail")?,
                    removable: row.get("entry_removable")?,
                    tree: if row.get::<_, Option<bool>>("entry_tree_present")? == Some(true) {
                        Some(crate::cleanup::Tree {
                            repo: rows::required(row, "entry_tree_repo")?,
                            path: rows::required(row, "entry_tree_path")?,
                            branch: rows::required(row, "entry_tree_branch")?,
                            head: rows::required(row, "entry_tree_head")?,
                            identity: if row
                                .get::<_, Option<bool>>("entry_tree_identity_present")?
                                == Some(true)
                            {
                                Some(crate::worktree::Identity {
                                    common: rows::required(row, "entry_tree_identity_common")?
                                        .into(),
                                    gitdir: rows::required(row, "entry_tree_identity_gitdir")?
                                        .into(),
                                    generation: rows::required(
                                        row,
                                        "entry_tree_identity_generation",
                                    )?,
                                })
                            } else {
                                None
                            },
                        })
                    } else {
                        None
                    },
                    kind: rows::text(row, "entry_kind")?,
                    dirty_paths: load_report_applied_failure_entries_entry_dirty_paths(tx, id)?
                        .into_iter()
                        .map(|(_, _, value)| value)
                        .collect(),
                    dirty_count: optional_number::<usize>(row, "entry_dirty_count")?,
                },
                detail: rows::required(row, "detail")?,
            },
        ));
    }
    Ok(values)
}
fn put_report_applied_repositories_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(
        tx,
        "host_report_applied_repositories_dirty_paths",
        "row_key",
    )?;
    Ok(())
}

fn load_report_applied_repositories_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement=tx.prepare_cached("SELECT * FROM host_report_applied_repositories_dirty_paths WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_repositories(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::cleanup::Entry,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("class", Some(&value.class));
    columns.text("id", Some(&value.id));
    columns.text("detail", Some(&value.detail));
    columns.flag("removable", value.removable);
    columns.flag("tree_present", value.tree.is_some());
    if let Some(value) = &value.tree {
        columns.text("tree_repo", Some(&value.repo));
        columns.text("tree_path", Some(&value.path));
        columns.text("tree_branch", Some(&value.branch));
        columns.text("tree_head", Some(&value.head));
        columns.flag("tree_identity_present", value.identity.is_some());
        if let Some(value) = &value.identity {
            columns.text("tree_identity_common", Some(&path_text(&value.common)?));
            columns.text("tree_identity_gitdir", Some(&path_text(&value.gitdir)?));
            columns.text("tree_identity_generation", Some(&value.generation));
        }
    }
    if let Some(value) = &value.kind {
        columns.text("kind", Some(value));
    }
    if let Some(value) = &value.dirty_count {
        columns.number(
            "dirty_count",
            Some(u64::try_from(*value).map_err(|_| invalid("integer out of range"))?),
        )?;
    }
    columns.upsert(tx, "host_report_applied_repositories", "row_key")?;
    tx.execute(
        "DELETE FROM host_report_applied_repositories_dirty_paths WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.dirty_paths.iter()) {
        put_report_applied_repositories_dirty_paths(tx, id, position, "", child)?;
    }
    Ok(())
}

fn load_report_applied_repositories(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::cleanup::Entry)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_repositories WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::cleanup::Entry {
                class: rows::required(row, "class")?,
                id: rows::required(row, "id")?,
                detail: rows::required(row, "detail")?,
                removable: row.get("removable")?,
                tree: if row.get::<_, Option<bool>>("tree_present")? == Some(true) {
                    Some(crate::cleanup::Tree {
                        repo: rows::required(row, "tree_repo")?,
                        path: rows::required(row, "tree_path")?,
                        branch: rows::required(row, "tree_branch")?,
                        head: rows::required(row, "tree_head")?,
                        identity: if row.get::<_, Option<bool>>("tree_identity_present")?
                            == Some(true)
                        {
                            Some(crate::worktree::Identity {
                                common: rows::required(row, "tree_identity_common")?.into(),
                                gitdir: rows::required(row, "tree_identity_gitdir")?.into(),
                                generation: rows::required(row, "tree_identity_generation")?,
                            })
                        } else {
                            None
                        },
                    })
                } else {
                    None
                },
                kind: rows::text(row, "kind")?,
                dirty_paths: load_report_applied_repositories_dirty_paths(tx, id)?
                    .into_iter()
                    .map(|(_, _, value)| value)
                    .collect(),
                dirty_count: optional_number::<usize>(row, "dirty_count")?,
            },
        ));
    }
    Ok(values)
}
fn put_report_applied_landed_dirty_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(
        tx,
        "host_report_applied_landed_dirty_dirty_paths",
        "row_key",
    )?;
    Ok(())
}

fn load_report_applied_landed_dirty_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement=tx.prepare_cached("SELECT * FROM host_report_applied_landed_dirty_dirty_paths WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_landed_dirty(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::cleanup::Entry,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("class", Some(&value.class));
    columns.text("id", Some(&value.id));
    columns.text("detail", Some(&value.detail));
    columns.flag("removable", value.removable);
    columns.flag("tree_present", value.tree.is_some());
    if let Some(value) = &value.tree {
        columns.text("tree_repo", Some(&value.repo));
        columns.text("tree_path", Some(&value.path));
        columns.text("tree_branch", Some(&value.branch));
        columns.text("tree_head", Some(&value.head));
        columns.flag("tree_identity_present", value.identity.is_some());
        if let Some(value) = &value.identity {
            columns.text("tree_identity_common", Some(&path_text(&value.common)?));
            columns.text("tree_identity_gitdir", Some(&path_text(&value.gitdir)?));
            columns.text("tree_identity_generation", Some(&value.generation));
        }
    }
    if let Some(value) = &value.kind {
        columns.text("kind", Some(value));
    }
    if let Some(value) = &value.dirty_count {
        columns.number(
            "dirty_count",
            Some(u64::try_from(*value).map_err(|_| invalid("integer out of range"))?),
        )?;
    }
    columns.upsert(tx, "host_report_applied_landed_dirty", "row_key")?;
    tx.execute(
        "DELETE FROM host_report_applied_landed_dirty_dirty_paths WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.dirty_paths.iter()) {
        put_report_applied_landed_dirty_dirty_paths(tx, id, position, "", child)?;
    }
    Ok(())
}

fn load_report_applied_landed_dirty(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::cleanup::Entry)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_landed_dirty WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::cleanup::Entry {
                class: rows::required(row, "class")?,
                id: rows::required(row, "id")?,
                detail: rows::required(row, "detail")?,
                removable: row.get("removable")?,
                tree: if row.get::<_, Option<bool>>("tree_present")? == Some(true) {
                    Some(crate::cleanup::Tree {
                        repo: rows::required(row, "tree_repo")?,
                        path: rows::required(row, "tree_path")?,
                        branch: rows::required(row, "tree_branch")?,
                        head: rows::required(row, "tree_head")?,
                        identity: if row.get::<_, Option<bool>>("tree_identity_present")?
                            == Some(true)
                        {
                            Some(crate::worktree::Identity {
                                common: rows::required(row, "tree_identity_common")?.into(),
                                gitdir: rows::required(row, "tree_identity_gitdir")?.into(),
                                generation: rows::required(row, "tree_identity_generation")?,
                            })
                        } else {
                            None
                        },
                    })
                } else {
                    None
                },
                kind: rows::text(row, "kind")?,
                dirty_paths: load_report_applied_landed_dirty_dirty_paths(tx, id)?
                    .into_iter()
                    .map(|(_, _, value)| value)
                    .collect(),
                dirty_count: optional_number::<usize>(row, "dirty_count")?,
            },
        ));
    }
    Ok(values)
}
fn put_report_applied_unlinked_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_report_applied_unlinked_dirty_paths", "row_key")?;
    Ok(())
}

fn load_report_applied_unlinked_dirty_paths(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_unlinked_dirty_paths WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn put_report_applied_unlinked(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::cleanup::Entry,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("class", Some(&value.class));
    columns.text("id", Some(&value.id));
    columns.text("detail", Some(&value.detail));
    columns.flag("removable", value.removable);
    columns.flag("tree_present", value.tree.is_some());
    if let Some(value) = &value.tree {
        columns.text("tree_repo", Some(&value.repo));
        columns.text("tree_path", Some(&value.path));
        columns.text("tree_branch", Some(&value.branch));
        columns.text("tree_head", Some(&value.head));
        columns.flag("tree_identity_present", value.identity.is_some());
        if let Some(value) = &value.identity {
            columns.text("tree_identity_common", Some(&path_text(&value.common)?));
            columns.text("tree_identity_gitdir", Some(&path_text(&value.gitdir)?));
            columns.text("tree_identity_generation", Some(&value.generation));
        }
    }
    if let Some(value) = &value.kind {
        columns.text("kind", Some(value));
    }
    if let Some(value) = &value.dirty_count {
        columns.number(
            "dirty_count",
            Some(u64::try_from(*value).map_err(|_| invalid("integer out of range"))?),
        )?;
    }
    columns.upsert(tx, "host_report_applied_unlinked", "row_key")?;
    tx.execute(
        "DELETE FROM host_report_applied_unlinked_dirty_paths WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.dirty_paths.iter()) {
        put_report_applied_unlinked_dirty_paths(tx, id, position, "", child)?;
    }
    Ok(())
}

fn load_report_applied_unlinked(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::cleanup::Entry)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_report_applied_unlinked WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::cleanup::Entry {
                class: rows::required(row, "class")?,
                id: rows::required(row, "id")?,
                detail: rows::required(row, "detail")?,
                removable: row.get("removable")?,
                tree: if row.get::<_, Option<bool>>("tree_present")? == Some(true) {
                    Some(crate::cleanup::Tree {
                        repo: rows::required(row, "tree_repo")?,
                        path: rows::required(row, "tree_path")?,
                        branch: rows::required(row, "tree_branch")?,
                        head: rows::required(row, "tree_head")?,
                        identity: if row.get::<_, Option<bool>>("tree_identity_present")?
                            == Some(true)
                        {
                            Some(crate::worktree::Identity {
                                common: rows::required(row, "tree_identity_common")?.into(),
                                gitdir: rows::required(row, "tree_identity_gitdir")?.into(),
                                generation: rows::required(row, "tree_identity_generation")?,
                            })
                        } else {
                            None
                        },
                    })
                } else {
                    None
                },
                kind: rows::text(row, "kind")?,
                dirty_paths: load_report_applied_unlinked_dirty_paths(tx, id)?
                    .into_iter()
                    .map(|(_, _, value)| value)
                    .collect(),
                dirty_count: optional_number::<usize>(row, "dirty_count")?,
            },
        ));
    }
    Ok(values)
}
fn report_row(tx: &Transaction<'_>, id: &str) -> Result<Option<crate::sweep::Report>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_report WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(crate::sweep::Report {
        ts_ms: number::<u64>(row, "ts_ms")?,
        started_ms: number::<u64>(row, "started_ms")?,
        finished_ms: number::<u64>(row, "finished_ms")?,
        applied: crate::cleanup::Applied {
            results: load_report_applied_results(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            failures: load_report_applied_failures(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            skipped: load_report_applied_skipped(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            failure_entries: load_report_applied_failure_entries(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            repositories: load_report_applied_repositories(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            landed_dirty: load_report_applied_landed_dirty(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
            unlinked: load_report_applied_unlinked(tx, id)?
                .into_iter()
                .map(|(_, _, value)| value)
                .collect(),
        },
    }))
}
fn put_report_row(tx: &Transaction<'_>, id: &str, value: &crate::sweep::Report) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.number("ts_ms", Some(value.ts_ms))?;
    columns.number("started_ms", Some(value.started_ms))?;
    columns.number("finished_ms", Some(value.finished_ms))?;
    columns.upsert(tx, "host_report", "row_key")?;
    tx.execute(
        "DELETE FROM host_report_applied_results WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.results.iter()) {
        put_report_applied_results(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_failures WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.failures.iter()) {
        put_report_applied_failures(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_skipped WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.skipped.iter()) {
        put_report_applied_skipped(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_failure_entries WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.failure_entries.iter()) {
        put_report_applied_failure_entries(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_repositories WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.repositories.iter()) {
        put_report_applied_repositories(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_landed_dirty WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.landed_dirty.iter()) {
        put_report_applied_landed_dirty(tx, id, position, "", child)?;
    }
    tx.execute(
        "DELETE FROM host_report_applied_unlinked WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.applied.unlinked.iter()) {
        put_report_applied_unlinked(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_marks_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::done::Mark,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("pane", Some(&value.pane));
    if let Some(value) = &value.item {
        columns.text("item", Some(value));
    }
    if let Some(value) = &value.session {
        columns.text("session", Some(value));
    }
    if let Some(value) = &value.name {
        columns.text("name", Some(value));
    }
    if let Some(value) = &value.after {
        columns.number("after", Some(*value))?;
    }
    if let Some(value) = &value.sealed {
        columns.number("sealed", Some(*value))?;
    }
    columns.number("marked_ms", Some(value.marked_ms))?;
    columns.flag("done_turn_present", value.done_turn.is_some());
    if let Some(value) = &value.done_turn {
        if let Some(value) = &value.state_change_seq {
            columns.number("done_turn_state_change_seq", Some(*value))?;
        }
        if let Some(value) = &value.state_since_ms {
            columns.number("done_turn_state_since_ms", Some(*value))?;
        }
    }
    columns.flag("signal_present", value.signal.is_some());
    if let Some(value) = &value.signal {
        columns.text("signal_kind", Some(&rows::name(&value.kind)?));
        columns.text("signal_text", Some(&value.text));
        columns.text("signal_event_id", Some(&value.event_id));
        if let Some(value) = &value.turn.state_change_seq {
            columns.number("signal_turn_state_change_seq", Some(*value))?;
        }
        if let Some(value) = &value.turn.state_since_ms {
            columns.number("signal_turn_state_since_ms", Some(*value))?;
        }
        columns.text("signal_session", Some(&value.session));
        columns.number("signal_recorded_ms", Some(value.recorded_ms))?;
        if let Some(value) = &value.cleared_ms {
            columns.number("signal_cleared_ms", Some(*value))?;
        }
    }
    columns.message("signal_history_message", Some(&value.signal_history))?;
    columns.message("signal_inputs_message", Some(&value.signal_inputs))?;
    columns.flag("signal_delivered", value.signal_delivered);
    columns.flag("branch_present", value.branch.is_some());
    if let Some(value) = &value.branch {
        columns.text("branch_repo", Some(&value.repo));
        columns.text("branch_name", Some(&value.name));
        columns.number("branch_ahead", Some(value.ahead))?;
        columns.flag("branch_landed", value.landed);
    }
    columns.flag("detached", value.detached);
    columns.flag("reply_present", value.reply.is_some());
    if let Some(value) = &value.reply {
        columns.number("reply_seq", Some(value.seq))?;
        if let Some(value) = &value.result_id {
            columns.text("reply_result_id", Some(value));
        }
        columns.text("reply_text", Some(&value.text));
        columns.number("reply_captured_ms", Some(value.captured_ms))?;
        if let Some(value) = &value.reported_ms {
            columns.number("reply_reported_ms", Some(*value))?;
        }
        columns.flag("reply_done", value.done);
    }
    columns.flag("native_done", value.native_done);
    if let Some(value) = &value.closed_ms {
        columns.number("closed_ms", Some(*value))?;
    }
    if let Some(value) = &value.dropped_ms {
        columns.number("dropped_ms", Some(*value))?;
    }
    columns.upsert(tx, "host_marks_entries", "row_key")?;
    Ok(())
}

fn load_marks_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::done::Mark)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_marks_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::done::Mark {
                pane: rows::required(row, "pane")?,
                item: rows::text(row, "item")?,
                session: rows::text(row, "session")?,
                name: rows::text(row, "name")?,
                after: optional_number::<u64>(row, "after")?,
                sealed: optional_number::<u64>(row, "sealed")?,
                marked_ms: number::<u64>(row, "marked_ms")?,
                done_turn: if row.get::<_, Option<bool>>("done_turn_present")? == Some(true) {
                    Some(crate::done::Turn {
                        state_change_seq: optional_number::<u64>(
                            row,
                            "done_turn_state_change_seq",
                        )?,
                        state_since_ms: optional_number::<u64>(row, "done_turn_state_since_ms")?,
                    })
                } else {
                    None
                },
                signal: if row.get::<_, Option<bool>>("signal_present")? == Some(true) {
                    Some(crate::done::Signal {
                        kind: rows::variant(rows::required(row, "signal_kind")?)?,
                        text: rows::required(row, "signal_text")?,
                        event_id: rows::required(row, "signal_event_id")?,
                        turn: crate::done::Turn {
                            state_change_seq: optional_number::<u64>(
                                row,
                                "signal_turn_state_change_seq",
                            )?,
                            state_since_ms: optional_number::<u64>(
                                row,
                                "signal_turn_state_since_ms",
                            )?,
                        },
                        session: rows::required(row, "signal_session")?,
                        recorded_ms: number::<u64>(row, "signal_recorded_ms")?,
                        cleared_ms: optional_number::<u64>(row, "signal_cleared_ms")?,
                    })
                } else {
                    None
                },
                signal_history: rows::decoded(row, "signal_history_message")?.unwrap_or_default(),
                signal_inputs: rows::decoded(row, "signal_inputs_message")?.unwrap_or_default(),
                signal_delivered: row.get("signal_delivered")?,
                branch: if row.get::<_, Option<bool>>("branch_present")? == Some(true) {
                    Some(crate::done::Branch {
                        repo: rows::required(row, "branch_repo")?,
                        name: rows::required(row, "branch_name")?,
                        ahead: number::<u64>(row, "branch_ahead")?,
                        landed: row.get("branch_landed")?,
                    })
                } else {
                    None
                },
                detached: row.get("detached")?,
                reply: if row.get::<_, Option<bool>>("reply_present")? == Some(true) {
                    Some(crate::items::Reply {
                        seq: number::<u64>(row, "reply_seq")?,
                        result_id: rows::text(row, "reply_result_id")?,
                        text: rows::required(row, "reply_text")?,
                        captured_ms: number::<u64>(row, "reply_captured_ms")?,
                        reported_ms: optional_number::<u64>(row, "reply_reported_ms")?,
                        done: row.get("reply_done")?,
                    })
                } else {
                    None
                },
                native_done: row.get("native_done")?,
                closed_ms: optional_number::<u64>(row, "closed_ms")?,
                dropped_ms: optional_number::<u64>(row, "dropped_ms")?,
            },
        ));
    }
    Ok(values
        .into_iter()
        .map(|(position, _, mark)| (position, crate::done::key(&mark), mark))
        .collect())
}
fn marks_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Marks>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_marks WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Marks {
        entries: load_marks_entries(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    }))
}
fn put_marks_row(tx: &Transaction<'_>, id: &str, value: &Marks) -> Result<()> {
    tx.execute(
        "INSERT INTO host_marks(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_marks_entries WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.entries.iter()) {
        put_marks_entries(tx, id, position, key, child)?;
    }
    Ok(())
}

fn put_trees_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::cleanup::Tree,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("repo", Some(&value.repo));
    columns.text("path", Some(&value.path));
    columns.text("branch", Some(&value.branch));
    columns.text("head", Some(&value.head));
    columns.flag("identity_present", value.identity.is_some());
    if let Some(value) = &value.identity {
        columns.text("identity_common", Some(&path_text(&value.common)?));
        columns.text("identity_gitdir", Some(&path_text(&value.gitdir)?));
        columns.text("identity_generation", Some(&value.generation));
    }
    columns.upsert(tx, "host_trees_entries", "row_key")?;
    Ok(())
}

fn load_trees_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::cleanup::Tree)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_trees_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::cleanup::Tree {
                repo: rows::required(row, "repo")?,
                path: rows::required(row, "path")?,
                branch: rows::required(row, "branch")?,
                head: rows::required(row, "head")?,
                identity: if row.get::<_, Option<bool>>("identity_present")? == Some(true) {
                    Some(crate::worktree::Identity {
                        common: rows::required(row, "identity_common")?.into(),
                        gitdir: rows::required(row, "identity_gitdir")?.into(),
                        generation: rows::required(row, "identity_generation")?,
                    })
                } else {
                    None
                },
            },
        ));
    }
    Ok(values)
}
fn trees_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Trees>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_trees WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Trees {
        entries: load_trees_entries(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_trees_row(tx: &Transaction<'_>, id: &str, value: &Trees) -> Result<()> {
    tx.execute(
        "INSERT INTO host_trees(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_trees_entries WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.entries.iter()) {
        put_trees_entries(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_repositories_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_repositories_entries", "row_key")?;
    Ok(())
}

fn load_repositories_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, String)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_repositories_entries WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn repositories_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Repositories>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_repositories WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Repositories {
        entries: load_repositories_entries(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_repositories_row(tx: &Transaction<'_>, id: &str, value: &Repositories) -> Result<()> {
    tx.execute(
        "INSERT INTO host_repositories(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute(
        "DELETE FROM host_repositories_entries WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.entries.iter()) {
        put_repositories_entries(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_custodies_entries_claims(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::custody::Claim,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("id", Some(&value.id));
    columns.text("claimant_pane", Some(&value.claimant_pane));
    if let Some(value) = &value.claimant_session {
        columns.text("claimant_session", Some(value));
    }
    columns.number("claimed_ms", Some(value.claimed_ms))?;
    if let Some(value) = &value.previous_custodian {
        columns.text("previous_custodian", Some(value));
    }
    columns.number("revision", Some(value.revision))?;
    columns.upsert(tx, "host_custodies_entries_claims", "row_key")?;
    Ok(())
}

fn load_custodies_entries_claims(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::custody::Claim)>> {
    let mut statement = tx.prepare_cached(
        "SELECT * FROM host_custodies_entries_claims WHERE owner=?1 ORDER BY position",
    )?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::custody::Claim {
                id: rows::required(row, "id")?,
                claimant_pane: rows::required(row, "claimant_pane")?,
                claimant_session: rows::text(row, "claimant_session")?,
                claimed_ms: number::<u64>(row, "claimed_ms")?,
                previous_custodian: rows::text(row, "previous_custodian")?,
                revision: number::<u64>(row, "revision")?,
            },
        ));
    }
    Ok(values)
}
fn put_custodies_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::custody::Custody,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("item", Some(&value.item));
    columns.text("host", Some(&value.host));
    columns.text("owner_pane", Some(&value.owner_pane));
    if let Some(value) = &value.owner_session {
        columns.text("owner_session", Some(value));
    }
    columns.text("custodian_pane", Some(&value.custodian_pane));
    if let Some(value) = &value.custodian_session {
        columns.text("custodian_session", Some(value));
    }
    columns.number("revision", Some(value.revision))?;
    columns.upsert(tx, "host_custodies_entries", "row_key")?;
    tx.execute(
        "DELETE FROM host_custodies_entries_claims WHERE owner = ?1",
        [id],
    )?;
    for (position, child) in (0_i64..).zip(value.claims.iter()) {
        put_custodies_entries_claims(tx, id, position, "", child)?;
    }
    Ok(())
}

fn load_custodies_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::custody::Custody)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_custodies_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::custody::Custody {
                item: rows::required(row, "item")?,
                host: rows::required(row, "host")?,
                owner_pane: rows::required(row, "owner_pane")?,
                owner_session: rows::text(row, "owner_session")?,
                custodian_pane: rows::required(row, "custodian_pane")?,
                custodian_session: rows::text(row, "custodian_session")?,
                revision: number::<u64>(row, "revision")?,
                claims: load_custodies_entries_claims(tx, id)?
                    .into_iter()
                    .map(|(_, _, value)| value)
                    .collect(),
            },
        ));
    }
    Ok(values)
}
fn custodies_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Custodies>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_custodies WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Custodies {
        entries: load_custodies_entries(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_custodies_row(tx: &Transaction<'_>, id: &str, value: &Custodies) -> Result<()> {
    tx.execute(
        "INSERT INTO host_custodies(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_custodies_entries WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.entries.iter()) {
        put_custodies_entries(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_parents_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::watchdog::Parent,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    if let Some(value) = &value.transcript {
        columns.text("transcript", Some(value));
    }
    columns.upsert(tx, "host_parents_entries", "row_key")?;
    Ok(())
}

fn load_parents_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::watchdog::Parent)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_parents_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            crate::watchdog::Parent {
                transcript: rows::text(row, "transcript")?,
            },
        ));
    }
    Ok(values)
}
fn parents_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Parents>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_parents WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Parents {
        entries: load_parents_entries(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    }))
}
fn put_parents_row(tx: &Transaction<'_>, id: &str, value: &Parents) -> Result<()> {
    tx.execute(
        "INSERT INTO host_parents(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_parents_entries WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.entries.iter()) {
        put_parents_entries(tx, id, position, key, child)?;
    }
    Ok(())
}

fn put_landing_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &str,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.text("value", Some(value));
    columns.upsert(tx, "host_landing_entries", "row_key")?;
    Ok(())
}

fn load_landing_entries(tx: &Transaction<'_>, owner: &str) -> Result<Vec<(i64, String, String)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_landing_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            rows::required(row, "map_key")?,
            rows::required(row, "value")?,
        ));
    }
    Ok(values)
}
fn landing_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Landing>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_landing WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Landing {
        entries: load_landing_entries(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    }))
}
fn put_landing_row(tx: &Transaction<'_>, id: &str, value: &Landing) -> Result<()> {
    tx.execute(
        "INSERT INTO host_landing(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_landing_entries WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.entries.iter()) {
        put_landing_entries(tx, id, position, key, child)?;
    }
    Ok(())
}

fn put_orphans_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::sweep::Orphan,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("item", Some(&value.item));
    columns.text("host", Some(&value.host));
    columns.text("pane", Some(&value.pane));
    columns.text("owner_pane", Some(&value.owner_pane));
    if let Some(value) = &value.name {
        columns.text("name", Some(value));
    }
    columns.text("reason", Some(&value.reason));
    columns.number("since_ms", Some(value.since_ms))?;
    columns.text("status", Some(&rows::name(&value.status)?));
    columns.flag("claimed", value.claimed);
    if let Some(value) = &value.custodian_pane {
        columns.text("custodian_pane", Some(value));
    }
    columns.upsert(tx, "host_orphans_entries", "row_key")?;
    Ok(())
}

fn load_orphans_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::sweep::Orphan)>> {
    let mut statement =
        tx.prepare_cached("SELECT * FROM host_orphans_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        values.push((
            row.get("position")?,
            String::new(),
            crate::sweep::Orphan {
                item: rows::required(row, "item")?,
                host: rows::required(row, "host")?,
                pane: rows::required(row, "pane")?,
                owner_pane: rows::required(row, "owner_pane")?,
                name: rows::text(row, "name")?,
                reason: rows::required(row, "reason")?,
                since_ms: number::<u64>(row, "since_ms")?,
                status: rows::variant(rows::required(row, "status")?)?,
                claimed: row.get("claimed")?,
                custodian_pane: rows::text(row, "custodian_pane")?,
            },
        ));
    }
    Ok(values)
}
fn orphans_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Orphans>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_orphans WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Orphans {
        entries: load_orphans_entries(tx, id)?
            .into_iter()
            .map(|(_, _, value)| value)
            .collect(),
    }))
}
fn put_orphans_row(tx: &Transaction<'_>, id: &str, value: &Orphans) -> Result<()> {
    tx.execute(
        "INSERT INTO host_orphans(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_orphans_entries WHERE owner = ?1", [id])?;
    for (position, child) in (0_i64..).zip(value.entries.iter()) {
        put_orphans_entries(tx, id, position, "", child)?;
    }
    Ok(())
}

fn put_operations_entries(
    tx: &Transaction<'_>,
    owner: &str,
    position: i64,
    key: &str,
    value: &crate::preservation::Operation,
) -> Result<()> {
    let id = format!("{}:{owner}/{position}", owner.len());
    let id = id.as_str();
    let _ = key;
    let mut columns = Columns::default();
    columns.text("row_key", Some(id));
    columns.text("owner", Some(owner));
    columns.put("position", Sql::Integer(position));
    columns.text("map_key", Some(key));
    columns.text("id", Some(&value.id));
    columns.text("expectation_pane", Some(&value.expectation.pane));
    if let Some(value) = &value.expectation.terminal {
        columns.text("expectation_terminal", Some(value));
    }
    columns.message(
        "expectation_request_message",
        value.expectation.request.as_ref(),
    )?;
    if let Some(value) = &value.expectation.launch {
        columns.text("expectation_launch", Some(value));
    }
    columns.message(
        "expectation_owners_message",
        Some(&value.expectation.owners),
    )?;
    columns.text("expectation_tree_repo", Some(&value.expectation.tree.repo));
    columns.text("expectation_tree_path", Some(&value.expectation.tree.path));
    columns.text(
        "expectation_tree_branch",
        Some(&value.expectation.tree.branch),
    );
    columns.text("expectation_tree_head", Some(&value.expectation.tree.head));
    columns.flag(
        "expectation_tree_identity_present",
        value.expectation.tree.identity.is_some(),
    );
    if let Some(value) = &value.expectation.tree.identity {
        columns.text(
            "expectation_tree_identity_common",
            Some(&path_text(&value.common)?),
        );
        columns.text(
            "expectation_tree_identity_gitdir",
            Some(&path_text(&value.gitdir)?),
        );
        columns.text(
            "expectation_tree_identity_generation",
            Some(&value.generation),
        );
    }
    match &value.expectation.mode {
        crate::preservation::CloseMode::Owner => columns.text("expectation_mode", Some("owner")),
        crate::preservation::CloseMode::StoppedOrphan { episode } => {
            columns.text("expectation_mode", Some("stopped_orphan"));
            columns.message("expectation_mode_episode_message", Some(episode))?;
        }
    }
    if let Some(value) = &value.expectation.caller.pane {
        columns.text("expectation_caller_pane", Some(value));
    }
    if let Some(value) = &value.expectation.caller.workspace {
        columns.text("expectation_caller_workspace", Some(value));
    }
    if let Some(value) = &value.expectation.caller.cwd {
        columns.text("expectation_caller_cwd", Some(value));
    }
    columns.text("initial_id", Some(&value.initial_id));
    if let Some(value) = &value.successor_id {
        columns.text("successor_id", Some(value));
    }
    columns.message("initial_message", value.initial.as_ref())?;
    columns.message("latest_message", value.latest.as_ref())?;
    columns.text("stage", Some(&rows::name(&value.stage)?));
    if let Some(value) = &value.blocker {
        columns.text("blocker", Some(value));
    }
    columns.message("result_message", value.result.as_ref())?;
    columns.upsert(tx, "host_operations_entries", "row_key")?;
    Ok(())
}

fn load_operations_entries(
    tx: &Transaction<'_>,
    owner: &str,
) -> Result<Vec<(i64, String, crate::preservation::Operation)>> {
    let mut statement = tx
        .prepare_cached("SELECT * FROM host_operations_entries WHERE owner=?1 ORDER BY position")?;
    let mut cursor = statement.query([owner])?;
    let mut values = Vec::new();
    while let Some(row) = cursor.next()? {
        let id = rows::required(row, "row_key")?;
        let id = id.as_str();
        let _ = (tx, id, row);
        let key: String = rows::required(row, "map_key")?;
        values.push((
            row.get("position")?,
            key.clone(),
            crate::preservation::Operation {
                key,
                id: rows::required(row, "id")?,
                expectation: crate::preservation::CloseExpectation {
                    pane: rows::required(row, "expectation_pane")?,
                    terminal: rows::text(row, "expectation_terminal")?,
                    request: rows::decoded(row, "expectation_request_message")?,
                    launch: rows::text(row, "expectation_launch")?,
                    owners: rows::decoded(row, "expectation_owners_message")?
                        .ok_or_else(|| invalid("missing evidence message"))?,
                    tree: crate::cleanup::Tree {
                        repo: rows::required(row, "expectation_tree_repo")?,
                        path: rows::required(row, "expectation_tree_path")?,
                        branch: rows::required(row, "expectation_tree_branch")?,
                        head: rows::required(row, "expectation_tree_head")?,
                        identity: if row
                            .get::<_, Option<bool>>("expectation_tree_identity_present")?
                            == Some(true)
                        {
                            Some(crate::worktree::Identity {
                                common: rows::required(row, "expectation_tree_identity_common")?
                                    .into(),
                                gitdir: rows::required(row, "expectation_tree_identity_gitdir")?
                                    .into(),
                                generation: rows::required(
                                    row,
                                    "expectation_tree_identity_generation",
                                )?,
                            })
                        } else {
                            None
                        },
                    },
                    mode: match rows::required(row, "expectation_mode")?.as_str() {
                        "owner" => crate::preservation::CloseMode::Owner,
                        "stopped_orphan" => crate::preservation::CloseMode::StoppedOrphan {
                            episode: rows::decoded(row, "expectation_mode_episode_message")?
                                .ok_or_else(|| invalid("missing episode"))?,
                        },
                        _ => return Err(invalid("invalid preservation mode")),
                    },
                    caller: crate::close::Caller {
                        pane: rows::text(row, "expectation_caller_pane")?,
                        workspace: rows::text(row, "expectation_caller_workspace")?,
                        cwd: rows::text(row, "expectation_caller_cwd")?,
                    },
                },
                initial_id: rows::required(row, "initial_id")?,
                successor_id: rows::text(row, "successor_id")?,
                initial: rows::decoded(row, "initial_message")?,
                latest: rows::decoded(row, "latest_message")?,
                stage: rows::variant(rows::required(row, "stage")?)?,
                blocker: rows::text(row, "blocker")?,
                result: rows::decoded(row, "result_message")?,
            },
        ));
    }
    Ok(values)
}
fn operations_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Operations>> {
    let mut statement = tx.prepare_cached("SELECT * FROM host_operations WHERE row_key=?1")?;
    let mut cursor = statement.query([id])?;
    let Some(row) = cursor.next()? else {
        return Ok(None);
    };
    let _ = (tx, id, row);
    Ok(Some(Operations {
        entries: load_operations_entries(tx, id)?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    }))
}
fn put_operations_row(tx: &Transaction<'_>, id: &str, value: &Operations) -> Result<()> {
    tx.execute(
        "INSERT INTO host_operations(row_key) VALUES (?1) ON CONFLICT(row_key) DO NOTHING",
        [id],
    )?;
    tx.execute("DELETE FROM host_operations_entries WHERE owner = ?1", [id])?;
    for (position, (key, child)) in (0_i64..).zip(value.entries.iter()) {
        put_operations_entries(tx, id, position, key, child)?;
    }
    Ok(())
}

pub(crate) fn snapshot(ledger: &Ledger) -> Result<Option<crate::node::Snapshot>> {
    crate::store::read(ledger, snapshot_rows)
}
pub(crate) fn snapshot_rows(tx: &Transaction<'_>) -> Result<Option<crate::node::Snapshot>> {
    snapshot_row(tx, "local")
}
pub(crate) fn put_snapshot(ledger: &Ledger, value: &crate::node::Snapshot) -> Result<()> {
    crate::store::write(ledger, |tx| replace_snapshot(tx, value))
}
pub(crate) fn replace_snapshot(tx: &Transaction<'_>, value: &crate::node::Snapshot) -> Result<()> {
    tx.execute("DELETE FROM host_snapshot WHERE row_key='local'", [])?;
    put_snapshot_row(tx, "local", value)
}
pub(crate) fn watchdog(ledger: &Ledger) -> Result<Option<crate::watchdog::State>> {
    crate::store::read(ledger, watchdog_rows)
}
pub(crate) fn watchdog_rows(tx: &Transaction<'_>) -> Result<Option<crate::watchdog::State>> {
    watchdog_row(tx, "local")
}
pub(crate) fn put_watchdog(ledger: &Ledger, value: &crate::watchdog::State) -> Result<()> {
    crate::store::write(ledger, |tx| replace_watchdog(tx, value))
}
pub(crate) fn replace_watchdog(tx: &Transaction<'_>, value: &crate::watchdog::State) -> Result<()> {
    tx.execute("DELETE FROM host_watchdog WHERE row_key='local'", [])?;
    put_watchdog_row(tx, "local", value)
}
pub(crate) fn peer(ledger: &Ledger, id: &str) -> Result<Option<crate::hosts::Cached>> {
    crate::store::read(ledger, |tx| peer_row(tx, id))
}
pub(crate) fn put_peer(ledger: &Ledger, id: &str, value: &crate::hosts::Cached) -> Result<()> {
    crate::store::write(ledger, |tx| {
        tx.execute("DELETE FROM host_peer WHERE row_key=?1", [id])?;
        put_peer_row(tx, id, value)
    })
}
pub(crate) fn quota(ledger: &Ledger) -> Result<Option<crate::quota::Snapshot>> {
    crate::store::read(ledger, quota_rows)
}
pub(crate) fn quota_rows(tx: &Transaction<'_>) -> Result<Option<crate::quota::Snapshot>> {
    quota_row(tx, "local")
}
pub(crate) fn put_quota(ledger: &Ledger, value: &crate::quota::Snapshot) -> Result<()> {
    crate::store::write(ledger, |tx| replace_quota(tx, value))
}
pub(crate) fn replace_quota(tx: &Transaction<'_>, value: &crate::quota::Snapshot) -> Result<()> {
    value.validate()?;
    tx.execute("DELETE FROM host_quota WHERE row_key='local'", [])?;
    put_quota_row(tx, "local", value)
}
pub(crate) fn quota_refresh(ledger: &Ledger) -> Result<Option<crate::quota::Refresh>> {
    crate::store::read(ledger, quota_refresh_rows)
}
pub(crate) fn quota_refresh_rows(tx: &Transaction<'_>) -> Result<Option<crate::quota::Refresh>> {
    quota_refresh_row(tx, "local")
}
pub(crate) fn put_quota_refresh(ledger: &Ledger, value: &crate::quota::Refresh) -> Result<()> {
    crate::store::write(ledger, |tx| replace_quota_refresh(tx, value))
}
pub(crate) fn replace_quota_refresh(
    tx: &Transaction<'_>,
    value: &crate::quota::Refresh,
) -> Result<()> {
    tx.execute("DELETE FROM host_quota_refresh WHERE row_key='local'", [])?;
    put_quota_refresh_row(tx, "local", value)
}
pub(crate) fn report(ledger: &Ledger) -> Result<Option<crate::sweep::Report>> {
    crate::store::read(ledger, report_rows)
}
/// The latest completed pass.
pub(crate) fn report_rows(tx: &Transaction<'_>) -> Result<Option<crate::sweep::Report>> {
    let key: Option<String> = tx
        .query_row(
            "SELECT row_key FROM host_report ORDER BY ts_ms DESC, rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            error => Err(error),
        })?;
    match key {
        Some(key) => report_row(tx, &key),
        None => Ok(None),
    }
}
/// Every retained pass, oldest first.
pub(crate) fn reports(ledger: &Ledger) -> Result<Vec<crate::sweep::Report>> {
    crate::store::read(ledger, |tx| {
        let keys: Vec<String> = tx
            .prepare("SELECT row_key FROM host_report ORDER BY ts_ms, rowid")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        keys.iter()
            .filter_map(|key| report_row(tx, key).transpose())
            .collect()
    })
}
/// Replace the pass history with one pass.
#[cfg(test)]
pub(crate) fn put_report(ledger: &Ledger, value: &crate::sweep::Report) -> Result<()> {
    crate::store::write(ledger, |tx| replace_report(tx, value))
}
pub(crate) fn replace_report(tx: &Transaction<'_>, value: &crate::sweep::Report) -> Result<()> {
    tx.execute("DELETE FROM host_report", [])?;
    put_report_row(tx, &crate::ledger::new_id(), value)
}
/// Append one completed pass and drop passes older than `keep_from_ms`.
pub(crate) fn record_report(
    ledger: &Ledger,
    value: &crate::sweep::Report,
    keep_from_ms: u64,
) -> Result<()> {
    let keep_from = i64::try_from(keep_from_ms).map_err(|_| invalid("time out of range"))?;
    crate::store::write(ledger, |tx| {
        put_report_row(tx, &crate::ledger::new_id(), value)?;
        tx.execute("DELETE FROM host_report WHERE ts_ms < ?1", [keep_from])?;
        Ok(())
    })
}
pub(crate) fn marks(ledger: &Ledger) -> Result<BTreeMap<String, crate::done::Mark>> {
    crate::store::read(ledger, marks_rows)
}
pub(crate) fn marks_rows(tx: &Transaction<'_>) -> Result<BTreeMap<String, crate::done::Mark>> {
    Ok(marks_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_marks(
    ledger: &Ledger,
    value: &BTreeMap<String, crate::done::Mark>,
) -> Result<()> {
    crate::store::write(ledger, |tx| replace_marks(tx, value))
}
pub(crate) fn replace_marks(
    tx: &Transaction<'_>,
    value: &BTreeMap<String, crate::done::Mark>,
) -> Result<()> {
    tx.execute("DELETE FROM host_marks WHERE row_key='local'", [])?;
    put_marks_row(
        tx,
        "local",
        &Marks {
            entries: value.clone(),
        },
    )
}
pub(crate) fn trees(ledger: &Ledger) -> Result<Vec<crate::cleanup::Tree>> {
    crate::store::read(ledger, trees_rows)
}
pub(crate) fn trees_rows(tx: &Transaction<'_>) -> Result<Vec<crate::cleanup::Tree>> {
    Ok(trees_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_trees(ledger: &Ledger, value: &[crate::cleanup::Tree]) -> Result<()> {
    crate::store::write(ledger, |tx| replace_trees(tx, value))
}
pub(crate) fn replace_trees(tx: &Transaction<'_>, value: &[crate::cleanup::Tree]) -> Result<()> {
    tx.execute("DELETE FROM host_trees WHERE row_key='local'", [])?;
    put_trees_row(
        tx,
        "local",
        &Trees {
            entries: value.to_vec(),
        },
    )
}
pub(crate) fn repositories(ledger: &Ledger) -> Result<BTreeSet<String>> {
    crate::store::read(ledger, repositories_rows)
}
pub(crate) fn repositories_rows(tx: &Transaction<'_>) -> Result<BTreeSet<String>> {
    Ok(repositories_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_repositories(ledger: &Ledger, value: &BTreeSet<String>) -> Result<()> {
    crate::store::write(ledger, |tx| replace_repositories(tx, value))
}
pub(crate) fn replace_repositories(tx: &Transaction<'_>, value: &BTreeSet<String>) -> Result<()> {
    tx.execute("DELETE FROM host_repositories WHERE row_key='local'", [])?;
    put_repositories_row(
        tx,
        "local",
        &Repositories {
            entries: value.clone(),
        },
    )
}
pub(crate) fn custodies(ledger: &Ledger) -> Result<Vec<crate::custody::Custody>> {
    crate::store::read(ledger, custodies_rows)
}
pub(crate) fn custodies_rows(tx: &Transaction<'_>) -> Result<Vec<crate::custody::Custody>> {
    Ok(custodies_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_custodies(ledger: &Ledger, value: &[crate::custody::Custody]) -> Result<()> {
    crate::store::write(ledger, |tx| replace_custodies(tx, value))
}
pub(crate) fn replace_custodies(
    tx: &Transaction<'_>,
    value: &[crate::custody::Custody],
) -> Result<()> {
    tx.execute("DELETE FROM host_custodies WHERE row_key='local'", [])?;
    put_custodies_row(
        tx,
        "local",
        &Custodies {
            entries: value.to_vec(),
        },
    )
}
pub(crate) fn parents(ledger: &Ledger) -> Result<BTreeMap<String, crate::watchdog::Parent>> {
    crate::store::read(ledger, parents_rows)
}
pub(crate) fn parents_rows(
    tx: &Transaction<'_>,
) -> Result<BTreeMap<String, crate::watchdog::Parent>> {
    Ok(parents_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_parents(
    ledger: &Ledger,
    value: &BTreeMap<String, crate::watchdog::Parent>,
) -> Result<()> {
    crate::store::write(ledger, |tx| replace_parents(tx, value))
}
pub(crate) fn replace_parents(
    tx: &Transaction<'_>,
    value: &BTreeMap<String, crate::watchdog::Parent>,
) -> Result<()> {
    tx.execute("DELETE FROM host_parents WHERE row_key='local'", [])?;
    put_parents_row(
        tx,
        "local",
        &Parents {
            entries: value.clone(),
        },
    )
}
pub(crate) fn landing(ledger: &Ledger) -> Result<BTreeMap<String, String>> {
    crate::store::read(ledger, landing_rows)
}
pub(crate) fn landing_rows(tx: &Transaction<'_>) -> Result<BTreeMap<String, String>> {
    Ok(landing_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_landing(ledger: &Ledger, value: &BTreeMap<String, String>) -> Result<()> {
    crate::store::write(ledger, |tx| replace_landing(tx, value))
}
pub(crate) fn replace_landing(
    tx: &Transaction<'_>,
    value: &BTreeMap<String, String>,
) -> Result<()> {
    tx.execute("DELETE FROM host_landing WHERE row_key='local'", [])?;
    put_landing_row(
        tx,
        "local",
        &Landing {
            entries: value.clone(),
        },
    )
}
pub(crate) fn orphans(ledger: &Ledger) -> Result<Vec<crate::sweep::Orphan>> {
    crate::store::read(ledger, orphans_rows)
}
pub(crate) fn orphans_rows(tx: &Transaction<'_>) -> Result<Vec<crate::sweep::Orphan>> {
    Ok(orphans_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_orphans(ledger: &Ledger, value: &[crate::sweep::Orphan]) -> Result<()> {
    crate::store::write(ledger, |tx| replace_orphans(tx, value))
}
pub(crate) fn replace_orphans(tx: &Transaction<'_>, value: &[crate::sweep::Orphan]) -> Result<()> {
    tx.execute("DELETE FROM host_orphans WHERE row_key='local'", [])?;
    put_orphans_row(
        tx,
        "local",
        &Orphans {
            entries: value.to_vec(),
        },
    )
}
pub(crate) fn operations(
    ledger: &Ledger,
) -> Result<BTreeMap<String, crate::preservation::Operation>> {
    crate::store::read(ledger, operations_rows)
}
pub(crate) fn operations_rows(
    tx: &Transaction<'_>,
) -> Result<BTreeMap<String, crate::preservation::Operation>> {
    Ok(operations_row(tx, "local")?
        .map(|v| v.entries)
        .unwrap_or_default())
}
pub(crate) fn put_operations(
    ledger: &Ledger,
    value: &BTreeMap<String, crate::preservation::Operation>,
) -> Result<()> {
    crate::store::write(ledger, |tx| replace_operations(tx, value))
}
pub(crate) fn replace_operations(
    tx: &Transaction<'_>,
    value: &BTreeMap<String, crate::preservation::Operation>,
) -> Result<()> {
    tx.execute("DELETE FROM host_operations WHERE row_key='local'", [])?;
    put_operations_row(
        tx,
        "local",
        &Operations {
            entries: value.clone(),
        },
    )
}

pub(crate) fn sweep_request(ledger: &Ledger) -> Result<Option<String>> {
    crate::store::read(ledger, |tx| {
        use rusqlite::OptionalExtension;
        Ok(tx
            .query_row(
                "SELECT token FROM host_sweep_request WHERE id = 'local'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    })
}
pub(crate) fn put_sweep_request(ledger: &Ledger, token: &str) -> Result<()> {
    crate::store::write(ledger, |tx| {
        let mut columns = Columns::default();
        columns.text("id", Some("local"));
        columns.text("token", Some(token));
        columns.upsert(tx, "host_sweep_request", "id")
    })
}
pub(crate) fn consume_sweep_request(ledger: &Ledger, token: &str) -> Result<()> {
    crate::store::write(ledger, |tx| {
        tx.execute(
            "DELETE FROM host_sweep_request WHERE id = 'local' AND token = ?1",
            [token],
        )?;
        Ok(())
    })
}

pub(crate) type Gate = BTreeMap<String, BTreeSet<BTreeSet<(String, Option<u64>, bool)>>>;
pub(crate) fn gate(tx: &Transaction<'_>) -> Result<Gate> {
    let mut gate = Gate::new();
    let owners: Vec<String> = tx
        .prepare_cached("SELECT id FROM host_gate ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for owner in owners {
        let positions: Vec<i64> = tx
            .prepare_cached(
                "SELECT position FROM host_gate_sets WHERE owner = ?1 ORDER BY position",
            )?
            .query_map([&owner], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut signatures = BTreeSet::new();
        for position in positions {
            let mut statement = tx.prepare_cached("SELECT item, ahead, dirty FROM host_gate_entries WHERE owner = ?1 AND signature_position = ?2 ORDER BY position")?;
            let mut cursor = statement.query(rusqlite::params![owner, position])?;
            let mut entries = BTreeSet::new();
            while let Some(row) = cursor.next()? {
                entries.insert((
                    row.get(0)?,
                    row.get::<_, Option<i64>>(1)?
                        .map(rows::unsigned)
                        .transpose()?,
                    row.get(2)?,
                ));
            }
            signatures.insert(entries);
        }
        gate.insert(owner, signatures);
    }
    Ok(gate)
}
pub(crate) fn replace_gate(tx: &Transaction<'_>, gate: &Gate) -> Result<()> {
    tx.execute("DELETE FROM host_gate", [])?;
    for (owner, signatures) in gate {
        tx.execute(
            "INSERT INTO host_gate(id) VALUES (?1) ON CONFLICT(id) DO NOTHING",
            [owner],
        )?;
        for (signature_position, signature) in (0_i64..).zip(signatures) {
            tx.execute(
                "INSERT INTO host_gate_sets(owner,position) VALUES (?1,?2)",
                rusqlite::params![owner, signature_position],
            )?;
            for (position, (item, ahead, dirty)) in (0_i64..).zip(signature) {
                tx.execute("INSERT INTO host_gate_entries(owner,signature_position,position,item,ahead,dirty) VALUES (?1,?2,?3,?4,?5,?6)",rusqlite::params![owner,signature_position,position,item,ahead.map(rows::integer).transpose()?,dirty])?;
            }
        }
    }
    Ok(())
}
