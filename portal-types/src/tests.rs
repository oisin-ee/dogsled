use super::*;

fn snapshot() -> PortalSnapshot {
    PortalSnapshot {
        version: PORTAL_VERSION,
        dogsled: "0.42.0".to_owned(),
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
        repos: Some(agents::RepoList::default()),
        autonomy: Some(autonomy::AutonomyView {
            fleet: vec!["netcup".to_owned()],
            unsupported: vec![],
        }),
        gaps: vec![],
    }
}

fn turn(id: &str, text: &str) -> agents::Turn {
    agents::Turn {
        id: id.into(),
        role: agents::Role::User,
        text: text.into(),
        source: agents::Source::Harness {
            harness: "pi".into(),
            model: None,
            provider: None,
        },
        timestamp: None,
        tool: None,
        end_turn: None,
    }
}

#[test]
fn conversation_splices_reconstruct_the_next_history() {
    let a = turn("a", "first");
    let b = turn("b", "second");
    let c = turn("c", "third");
    let d = turn("d", "fourth");
    let base = [a.clone(), b.clone(), c.clone()];
    for (next, expected) in [
        (vec![a.clone(), b.clone(), c.clone(), d.clone()], (0, 3)),
        (vec![a, turn("b", "edited"), c.clone()], (0, 1)),
        (vec![b, c, d.clone()], (1, 2)),
    ] {
        let (drop_front, keep) = live::diff(&base, &next).unwrap();
        assert_eq!((drop_front, keep), expected);
        assert_eq!(
            live::splice(&base, drop_front, keep, next[keep..].to_vec()),
            Some(next)
        );
    }
    assert_eq!(live::diff(&base, &[d]), None);
    assert_eq!(live::diff(&base, &[]), None);
    assert_eq!(live::splice(&base, 2, 2, vec![]), None);
    assert_eq!(live::splice(&base, usize::MAX, 1, vec![]), None);
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
fn catalogue_snapshot_version_two_preserves_absent_empty_and_failed_results() {
    assert_eq!(PORTAL_VERSION, 2);
    let choice = agents::RepoChoice {
        slug: "owner/repo".into(),
        description: Some("Retained".into()),
        pushed_at: None,
        private: false,
    };
    for repos in [
        None,
        Some(agents::RepoList::default()),
        Some(agents::RepoList {
            repos: vec![],
            error: Some("GitHub catalogue: failed".into()),
        }),
        Some(agents::RepoList {
            repos: vec![choice],
            error: Some("GitHub catalogue: failed".into()),
        }),
    ] {
        let mut snapshot = snapshot();
        snapshot.repos = repos;
        let frame = PortalEvent::Snapshot(Box::new(snapshot));
        let bytes = serde_json::to_vec(&frame).unwrap();
        assert_eq!(
            serde_json::from_slice::<PortalEvent>(&bytes).unwrap(),
            frame
        );
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["repos"] = serde_json::json!({"repos":"malformed","error":null});
        assert!(serde_json::from_value::<PortalEvent>(value).is_err());
    }
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
