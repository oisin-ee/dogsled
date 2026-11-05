//! Reader fixtures use the same wire builders as the producer, then its revision stamper.

use super::PortalState;
use crate::error::Result;
use crate::ledger::Ledger;
use crate::observation::{Domains, Publication, Publisher, Sources};
use portal_types::BoardView;
use portal_types::agents::AgentView;

pub(super) fn publish(
    publisher: &mut Publisher,
    ledger: &Ledger,
    board: BoardView,
    agents: Vec<AgentView>,
) -> Result<PortalState> {
    let inbox = super::inbox::view(
        &board,
        &super::reviews::pending(ledger)?,
        &super::store::pending(ledger)?,
        &agents,
        board.generated_ms,
    );
    let snapshot = portal_types::PortalSnapshot {
        version: crate::observation::VERSION,
        epoch: String::new(),
        revision: 0,
        published_ms: board.generated_ms,
        board: Some(board),
        agents: Some(agents),
        inbox: Some(inbox),
        gaps: vec![],
    };
    let frame = publisher.replace(Publication {
        published_ms: snapshot.published_ms,
        sources: Sources::default(),
        local: Domains::new(),
        views: Domains::from([(
            "portal".into(),
            serde_json::to_value(crate::observation::project::View {
                data: Some(snapshot),
                gaps: vec![],
            })?,
        )]),
        gaps: vec![],
    })?;
    Ok(PortalState {
        snapshot: Some(crate::observation::read::view(&frame, "portal")?),
        error: None,
    })
}

pub(super) fn empty(ledger: &Ledger) -> PortalState {
    let board = super::board::view(
        &crate::board::Board {
            hosts: vec![],
            rows: vec![],
        },
        &[],
        &[],
        7,
    );
    publish(
        &mut Publisher::new().expect("publisher"),
        ledger,
        board,
        vec![],
    )
    .expect("producer wire projection")
}
