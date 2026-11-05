use super::*;

fn snapshot() -> PortalSnapshot {
    PortalSnapshot {
        version: 1,
        epoch: "service".to_owned(),
        revision: 3,
        published_ms: 7,
        board: Some(BoardView {
            hosts: vec![],
            repos: vec![],
            generated_ms: 7,
        }),
        agents: Some(vec![]),
        inbox: Some(inbox::InboxView::default()),
        gaps: vec![],
    }
}

#[test]
fn portal_snapshot_round_trips_only_as_snapshot() -> Result<(), serde_json::Error> {
    let frame = PortalEvent::Snapshot(Box::new(snapshot()));
    let bytes = serde_json::to_vec(&frame)?;
    assert_eq!(serde_json::from_slice::<PortalEvent>(&bytes)?, frame);
    assert_eq!(
        serde_json::from_slice::<PortalSnapshot>(&bytes)?,
        snapshot()
    );
    assert!(serde_json::from_slice::<PortalStatus>(&bytes).is_err());
    Ok(())
}

#[test]
fn portal_status_round_trips_only_as_status() -> Result<(), serde_json::Error> {
    for observation in [
        PortalAvailability::Unavailable,
        PortalAvailability::Disconnected,
    ] {
        let status = PortalStatus {
            observation,
            error: "observation unavailable".to_owned(),
        };
        let frame = PortalEvent::Status(status.clone());
        let bytes = serde_json::to_vec(&frame)?;
        assert_eq!(serde_json::from_slice::<PortalEvent>(&bytes)?, frame);
        assert_eq!(serde_json::from_slice::<PortalStatus>(&bytes)?, status);
        assert!(serde_json::from_slice::<PortalSnapshot>(&bytes).is_err());
    }
    Ok(())
}

#[test]
fn portal_frames_reject_ambiguous_shapes() -> Result<(), serde_json::Error> {
    let mut value = serde_json::to_value(snapshot())?;
    value["observation"] = serde_json::json!("disconnected");
    value["error"] = serde_json::json!("disconnected");
    assert!(serde_json::from_value::<PortalSnapshot>(value.clone()).is_err());
    assert!(serde_json::from_value::<PortalStatus>(value.clone()).is_err());
    assert!(serde_json::from_value::<PortalEvent>(value).is_err());
    assert!(
        serde_json::from_value::<PortalEvent>(serde_json::json!({"error":"missing discriminator"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<PortalEvent>(
            serde_json::json!({"observation":"working","error":"unknown state"})
        )
        .is_err()
    );
    Ok(())
}
