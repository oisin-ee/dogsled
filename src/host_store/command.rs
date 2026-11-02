//! Hidden, typed fixture and inspection commands. JSON exists only at the CLI boundary.

use super::{
    BTreeMap, BTreeSet, Ledger, Result, consume_sweep_request, custodies, gate, landing, marks,
    operations, orphans, parents, peer, put_custodies, put_landing, put_marks, put_operations,
    put_orphans, put_parents, put_peer, put_quota, put_quota_refresh, put_report, put_repositories,
    put_snapshot, put_sweep_request, put_trees, put_watchdog, quota, quota_refresh, replace_gate,
    report, repositories, snapshot, sweep_request, trees, watchdog,
};

macro_rules! commands {
    ($($variant:ident => $table:literal, $ty:ty, $get:ident, $put:ident;)+) => {
        #[derive(Debug, clap::Subcommand)]
        pub(crate) enum Command {
            Peer { id: String, #[arg(long, conflicts_with = "delete")] put: bool, #[arg(long)] delete: bool },
            SweepRequest { #[arg(long, conflicts_with = "delete")] put: bool, #[arg(long)] delete: bool },
            Gate { #[arg(long)] delete: bool },
            $( $variant {
                #[arg(long, conflicts_with = "delete")]
                put: bool,
                #[arg(long)]
                delete: bool,
            }, )+
        }
        pub(crate) fn run(command: Command, ledger: &Ledger) -> Result<serde_json::Value> {
            match command {
                Command::Peer { id, put, delete } => {
                    if put { let value = serde_json::from_reader(std::io::stdin().lock())?; put_peer(ledger, &id, &value)?; }
                    if delete { crate::store::write(ledger, |tx| { tx.execute("DELETE FROM host_peer WHERE row_key = ?1", [&id])?; Ok(()) })?; }
                    Ok(serde_json::to_value(peer(ledger, &id)?)?)
                }
                Command::SweepRequest { put, delete } => {
                    if put { let token: String = serde_json::from_reader(std::io::stdin().lock())?; put_sweep_request(ledger, &token)?; }
                    if delete && let Some(token) = sweep_request(ledger)? { consume_sweep_request(ledger, &token)?; }
                    Ok(serde_json::to_value(sweep_request(ledger)?)?)
                }
                Command::Gate { delete } => {
                    crate::store::write(ledger, |tx| {
                        if delete { replace_gate(tx, &Default::default())?; }
                        Ok(serde_json::to_value(gate(tx)?)?)
                    })
                }
                $(Command::$variant { put, delete } => {
                    if put {
                        let value: $ty = serde_json::from_reader(std::io::stdin().lock())?;
                        $put(ledger, &value)?;
                    } else if delete {
                        crate::store::write(ledger, |tx| {
                            tx.execute(concat!("DELETE FROM ", $table, " WHERE row_key = 'local'"), [])?;
                            Ok(())
                        })?;
                    }
                    Ok(serde_json::to_value($get(ledger)?)?)
                },)+
            }
        }
    }
}

pub(crate) fn fault(ledger: &Ledger, store: &str, write: bool) -> Result<bool> {
    // The quota cache read fails while its table is renamed away, and recovers when it is back.
    let rename = match store {
        "quota" => Some("ALTER TABLE host_quota RENAME TO host_quota_faulted"),
        "clear-quota" => Some("ALTER TABLE host_quota_faulted RENAME TO host_quota"),
        _ => None,
    };
    if let Some(statement) = rename {
        crate::store::write(ledger, |tx| Ok(tx.execute_batch(statement)?))?;
        return Ok(true);
    }
    let table = match store {
        "host" => "host_snapshot",
        "done" => "host_marks",
        "custody" => "host_custodies",
        "preservation" => "host_operations_entries",
        "sweep-request" => "host_sweep_request",
        _ => return Ok(false),
    };
    crate::store::write(ledger, |tx| {
        if write {
            tx.execute_batch(&format!("CREATE TRIGGER refuse_{table} BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'host store write refused'); END;"))?;
        } else {
            tx.execute_batch(&format!("DROP TABLE {table}"))?;
        }
        Ok(())
    })?;
    Ok(true)
}

commands! {
    Host => "host_snapshot", crate::node::Snapshot, snapshot, put_snapshot;
    Done => "host_marks", BTreeMap<String, crate::done::Mark>, marks, put_marks;
    Watchdog => "host_watchdog", crate::watchdog::State, watchdog, put_watchdog;
    Worktrees => "host_trees", Vec<crate::cleanup::Tree>, trees, put_trees;
    Repositories => "host_repositories", BTreeSet<String>, repositories, put_repositories;
    Parents => "host_parents", BTreeMap<String, crate::watchdog::Parent>, parents, put_parents;
    Custody => "host_custodies", Vec<crate::custody::Custody>, custodies, put_custodies;
    Landing => "host_landing", BTreeMap<String, String>, landing, put_landing;
    Orphans => "host_orphans", Vec<crate::sweep::Orphan>, orphans, put_orphans;
    PreservationOperations => "host_operations", BTreeMap<String, crate::preservation::Operation>, operations, put_operations;
    Sweep => "host_report", crate::sweep::Report, report, put_report;
    QuotaSnapshot => "host_quota", crate::quota::Snapshot, quota, put_quota;
    QuotaRefresh => "host_quota_refresh", crate::quota::Refresh, quota_refresh, put_quota_refresh;
}
