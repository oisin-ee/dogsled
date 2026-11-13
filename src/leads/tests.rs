use super::*;
use crate::error::Error;
use crate::lead_store;
use crate::ledger::Ledger;
use crate::probe::AgentCounts;
use crate::shell::Fake;
use serde_json::json;

fn ledger() -> Ledger {
    Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
}

fn record(id: &str) -> Record {
    Record {
        id: id.into(),
        name: format!("lead-{id}"),
        repository: Repository {
            path: "/home/repo".into(),
            origin: "https://github.com/oisin-ee/dogsled".into(),
            relative: "repo".into(),
            working_root: "/home/repo".into(),
        },
        prompt: "  first prompt\n".into(),
        validity: Validity::new(100),
        created_ms: 100,
        phase: Phase::Recorded,
        pane: None,
        terminal: None,
        workspace: None,
        updated_ms: 100,
        error: None,
        close_requested_ms: None,
        closed_ms: None,
    }
}

#[test]
fn peer_delivery_without_model_settings_still_parses() {
    let delivery: super::routing::Delivery = serde_json::from_value(json!({
        "version": 1, "id": "request", "repo": "owner/repo", "prompt": "go",
        "validity": Validity::new(100), "host": "netcup"
    }))
    .unwrap();
    assert_eq!(delivery.model, None);
    assert_eq!(delivery.thinking, None);
    let mut wire = serde_json::to_value(delivery).unwrap();
    assert!(wire.get("model").is_none());
    wire["model"] = json!({"provider":"openai", "id":"gpt-5"});
    wire["thinking"] = json!("high");
    let selected: super::routing::Delivery = serde_json::from_value(wire).unwrap();
    assert_eq!(selected.model.unwrap().pattern(), "openai/gpt-5");
    assert_eq!(selected.thinking, Some(ThinkingLevel::High));
}

#[test]
fn lead_launch_passes_model_settings_to_native_pi() {
    let ledger = ledger();
    std::fs::create_dir_all(&ledger.dir).unwrap();
    let path = ledger.dir.display().to_string();
    let shell = Fake::new();
    shell
        .on(&format!("git -C {path} rev-parse --show-toplevel"), &path)
        .on(
            &format!("herdr worktree list --cwd {path}"),
            r#"{"result":{"worktrees":[]}}"#,
        )
        .on(
            &format!("herdr workspace create --cwd {path} --no-focus"),
            r#"{"result":{"root_pane":{"pane_id":"w1:p1","terminal_id":"lead-terminal"}}}"#,
        );
    lead_panes(&shell, "lead-terminal", "idle");
    let server = crate::herdr_socket::fake::Server::start(|_, _| json!({"result":{}}));
    let socket = server.client();
    let context = CreateContext {
        shell: &shell,
        ledger: &ledger,
        config_path: std::path::Path::new("unused"),
        home: &path,
        herdr: Some(&socket),
    };
    let model = ModelRef {
        provider: "openai".into(),
        id: "gpt-5".into(),
    };
    for (id, selected, thinking, suffix) in [
        ("defaults", None, None, ""),
        (
            "selected",
            Some(&model),
            Some(ThinkingLevel::High),
            " -- --model openai/gpt-5 --thinking high",
        ),
        (
            "thinking",
            None,
            Some(ThinkingLevel::Off),
            " -- --thinking off",
        ),
        ("model", Some(&model), None, " -- --model openai/gpt-5"),
    ] {
        let mut saved = record(id);
        saved.repository.path.clone_from(&path);
        saved.validity = Validity::new(crate::ledger::now_ms());
        lead_store::insert(&ledger, &saved).unwrap();
        let command = format!("herdr agent start lead-{id} --kind pi --pane w1:p1{suffix}");
        shell.on(&command, r#"{"result":{}}"#);
        launch(&context, &mut saved, selected, thinking).unwrap();
        assert_eq!(saved.phase, Phase::Ready);
        assert!(calls(&shell).contains(&command));
    }
    assert_eq!(server.methods(), vec!["agent.prompt"; 4]);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lead_records_round_trip() {
    let ledger = ledger();
    let mut saved = record("request");
    lead_store::insert(&ledger, &saved).unwrap();
    assert!(lead_store::insert(&ledger, &saved).is_err());
    assert!(lead_store::update(&ledger, "absent", |_| Ok(())).is_err());
    for phase in [
        Phase::Allocating,
        Phase::Allocated,
        Phase::Starting,
        Phase::Started,
        Phase::Prompting,
        Phase::Ready,
    ] {
        lead_store::update(&ledger, &saved.id, |record| {
            record.phase = phase;
            record.pane = Some("w1:p1".into());
            record.terminal = Some("terminal".into());
            record.workspace = Some("w1".into());
            record.updated_ms += 1;
            Ok(())
        })
        .unwrap();
        saved.phase = phase;
        saved.pane = Some("w1:p1".into());
        saved.terminal = Some("terminal".into());
        saved.workspace = Some("w1".into());
        saved.updated_ms += 1;
        crate::store::release();
        let reopened = Ledger::new(ledger.dir.clone());
        assert_eq!(
            json!(lead_store::get(&reopened, &saved.id).unwrap()),
            json!(saved)
        );
    }
    lead_store::update(&ledger, &saved.id, |record| {
        record.error = Some("prompt connection failed".into());
        record.close_requested_ms = Some(200);
        record.closed_ms = Some(201);
        Ok(())
    })
    .unwrap();
    saved.error = Some("prompt connection failed".into());
    saved.close_requested_ms = Some(200);
    saved.closed_ms = Some(201);
    crate::store::release();
    assert_eq!(json!(lead_store::list(&ledger).unwrap()), json!([saved]));
    assert!(crate::items::list(&ledger).unwrap().is_empty());
    assert!(crate::work::queue(&ledger).unwrap().is_empty());
    assert_eq!(crate::job_store::reservations(&ledger).unwrap(), []);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lead_launch_intent_cannot_be_replaced() {
    let ledger = ledger();
    let saved = record("request");
    lead_store::insert(&ledger, &saved).unwrap();
    for field in [
        "id", "name", "repo", "origin", "relative", "root", "prompt", "validity", "created",
    ] {
        let error = lead_store::update(&ledger, &saved.id, |record| {
            record.phase = Phase::Starting;
            match field {
                "id" => record.id = "another".into(),
                "name" => record.name = "another".into(),
                "repo" => record.repository.path = "/another".into(),
                "origin" => record.repository.origin = "another".into(),
                "relative" => record.repository.relative = "another".into(),
                "root" => record.repository.working_root = "/another".into(),
                "prompt" => record.prompt = "another".into(),
                "validity" => record.validity.created_ms += 1,
                "created" => record.created_ms += 1,
                _ => unreachable!(),
            }
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(error, Error::Invalid(_)), "{field}: {error}");
        assert_eq!(
            json!(lead_store::get(&ledger, &saved.id).unwrap()),
            json!(saved)
        );
    }
    let mut invalid = record("another");
    invalid.name.clone_from(&saved.name);
    assert!(lead_store::insert(&ledger, &invalid).is_err());
    assert!(lead_store::get(&ledger, "another").unwrap().is_none());
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lead_identity_requires_terminal() {
    let mut saved = record("request");
    saved.pane = Some("w1:p1".into());
    let mut agent = crate::probe::Agent {
        pane: "w1:p1".into(),
        name: Some(saved.name.clone()),
        ..crate::probe::Agent::default()
    };
    assert!(!matches_agent(&saved, &agent));
    assert!(protects_pane(&saved, &agent.pane, None));
    agent.terminal_id = Some("terminal".into());
    assert!(!matches_agent(&saved, &agent));
    assert!(protects_pane(
        &saved,
        &agent.pane,
        agent.terminal_id.as_deref()
    ));
    saved.terminal = Some("terminal".into());
    assert!(matches_agent(&saved, &agent));
    agent.terminal_id = None;
    assert!(!matches_agent(&saved, &agent));
    assert!(protects_pane(&saved, &agent.pane, None));
    agent.terminal_id = Some("replacement".into());
    assert!(!matches_agent(&saved, &agent));
    assert!(!protects_pane(
        &saved,
        &agent.pane,
        agent.terminal_id.as_deref()
    ));
    agent.terminal_id = Some("terminal".into());
    agent.pane = "w2:p1".into();
    assert!(!matches_agent(&saved, &agent));
    assert!(!protects_pane(&saved, &agent.pane, None));
    agent.pane = "w1:p1".into();
    saved.closed_ms = Some(200);
    assert!(!matches_agent(&saved, &agent));
    assert!(!protects_pane(&saved, &agent.pane, None));
}

fn observed_shell() -> Fake {
    let shell = Fake::new();
    shell
        .on("herdr pane list", r#"{"result":{"panes":[]}}"#)
        .on("read /proc/loadavg", "0.5 0.2 0.1 1/10 42")
        .on("read /proc/stat", "cpu 10 0 10 80 0 0 0 0\n")
        .on(
            "read /proc/meminfo",
            "MemTotal: 1000 kB\nMemAvailable: 400 kB\n",
        )
        .on("df -Pk /home", "FS 1000 500 400 60% /");
    shell
}

#[test]
fn lead_counts_exclude_only_registered_agents() {
    let ledger = ledger();
    for (id, pane, terminal) in [
        ("exact", "w1:p1", Some("lead-terminal")),
        ("reused", "w1:p2", Some("old-terminal")),
        ("unknown", "w1:p3", None),
        ("unobserved", "w1:p4", Some("terminal")),
        ("closed", "w1:p5", Some("closed-terminal")),
    ] {
        let mut saved = record(id);
        saved.pane = Some(pane.into());
        saved.terminal = terminal.map(str::to_owned);
        if id == "closed" {
            saved.closed_ms = Some(200);
        }
        lead_store::insert(&ledger, &saved).unwrap();
    }
    crate::store::release();
    let shell = observed_shell();
    shell.on("herdr agent list", &json!({"result":{"agents":[
        {"pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"lead-terminal"},
        {"pane_id":"w1:p2","agent":"pi","agent_status":"working","terminal_id":"new-terminal","name":"lead-reused"},
        {"pane_id":"w1:p3","agent":"pi","agent_status":"working","terminal_id":"unknown-terminal"},
        {"pane_id":"w1:p4","agent":"pi","agent_status":"idle"},
        {"pane_id":"w1:p5","agent":"pi","agent_status":"working","terminal_id":"closed-terminal"},
        {"pane_id":"w2:p1","agent":"pi","agent_status":"done","terminal_id":"child-terminal"}
    ]}}).to_string());
    let raw = crate::probe::local(&shell, "/home").unwrap();
    let observed = crate::node::observed_probe(&ledger, &shell, "/home").unwrap();
    assert_eq!(raw.agents.total, 6);
    assert_eq!(observed.agent_panes.len(), 6);
    assert_eq!(AgentCounts::from_agents(&observed.agent_panes), raw.agents);
    assert_eq!(observed.agents.total, 5);
    assert_eq!(observed.agents.active(), 3);
    assert_eq!(observed.agents.by_status["working"], 3);
    assert_eq!(observed.agents.by_status["idle"], 1);
    assert_eq!(observed.agents.by_status["done"], 1);
    assert_eq!(observed.memory, raw.memory);
    assert_eq!(observed.cpu_ticks, raw.cpu_ticks);
    assert_eq!(observed.load1, raw.load1);
    assert_eq!(observed.busy, raw.busy);
    assert_eq!(observed.disk_free_percent, raw.disk_free_percent);
    assert_eq!(observed.reservations, raw.reservations);
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lead_validity_survives_request_retention() {
    let ledger = ledger();
    let mut saved = record("request");
    saved.closed_ms = Some(200);
    lead_store::insert(&ledger, &saved).unwrap();
    crate::store::write(&ledger, |tx| {
        crate::job_store::put_validity(tx, &saved.id, saved.validity)?;
        crate::job_store::put_validity(tx, "unreferenced", saved.validity)
    })
    .unwrap();
    crate::work::expire_requests(&ledger, 201 + crate::jobs::RECORD_KEEP_MS).unwrap();
    assert_eq!(
        crate::job_store::validities(&ledger).unwrap(),
        [(saved.id.clone(), saved.validity)]
    );
    assert_eq!(
        json!(lead_store::get(&ledger, &saved.id).unwrap()),
        json!(saved)
    );
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

/// A lead on `w1:p1` whose stored terminal is `lead-terminal`, in a workspace of its own.
fn opened(ledger: &Ledger) -> Record {
    let mut saved = record("open");
    saved.phase = Phase::Ready;
    saved.pane = Some("w1:p1".into());
    saved.terminal = Some("lead-terminal".into());
    saved.workspace = Some("w1".into());
    lead_store::insert(ledger, &saved).unwrap();
    saved
}

fn lead_panes(shell: &Fake, terminal: &str, status: &str) {
    shell
        .on(
            "herdr pane list",
            &json!({"result":{"panes":[
                {"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":status,
                 "cwd":"/home/repo","terminal_id":terminal},
                {"pane_id":"w1:p2","workspace_id":"w1","cwd":"/home/repo","terminal_id":"shell"}
            ]}})
            .to_string(),
        )
        .on(
            "herdr workspace list",
            r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"repo"}]}}"#,
        )
        .on("herdr pane close w1:p1", r#"{"result":{}}"#);
}

fn calls(shell: &Fake) -> Vec<String> {
    shell.calls.lock().unwrap().clone()
}

#[test]
fn lead_close_preserves_repository() {
    let ledger = ledger();
    let saved = opened(&ledger);
    let shell = Fake::new();
    lead_panes(&shell, "lead-terminal", "idle");
    let closed = crate::close::local(
        &shell,
        &ledger,
        "w1:p1",
        &crate::close::Caller::default(),
        crate::close::Guard::Clean,
    )
    .unwrap();
    assert_eq!(closed.pane, "w1:p1");
    assert!(closed.pane_closed);
    assert_eq!(closed.worktree, None);
    assert_eq!(closed.retained_branch, None);
    // Only the pane closed: no Git, teardown, workspace close, or checkout removal.
    let ran = calls(&shell);
    assert!(
        ran.contains(&"herdr pane close w1:p1".to_owned()),
        "{ran:?}"
    );
    assert!(
        ran.iter().all(|call| call.starts_with("herdr pane list")
            || call == "herdr workspace list"
            || call == "herdr pane close w1:p1"),
        "{ran:?}"
    );
    let record = lead_store::get(&ledger, &saved.id).unwrap().unwrap();
    assert!(record.close_requested_ms.is_some() && record.closed_ms.is_some());
    assert_eq!(record.phase, Phase::Ready);
    // Automatic guards never reach the lead, even before it closed.
    let ledger_two = self::ledger();
    opened(&ledger_two);
    for guard in [
        crate::close::Guard::Pushed,
        crate::close::Guard::Reader,
        crate::close::Guard::Writer,
    ] {
        let shell = Fake::new();
        lead_panes(&shell, "lead-terminal", "idle");
        let error = crate::close::local(
            &shell,
            &ledger_two,
            "w1:p1",
            &crate::close::Caller::default(),
            guard,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("closes only explicitly"),
            "{error}"
        );
        assert!(!calls(&shell).iter().any(|call| call.contains("close")));
    }
    std::fs::remove_dir_all(ledger.dir).unwrap();
    std::fs::remove_dir_all(ledger_two.dir).unwrap();
}

#[test]
fn lead_close_refuses_replaced_terminal() {
    let ledger = ledger();
    let saved = opened(&ledger);
    for (terminal, status, caller, expected) in [
        (
            "replacement",
            "idle",
            None,
            "terminal identity does not prove",
        ),
        ("lead-terminal", "working", None, "is busy"),
        ("lead-terminal", "idle", Some("w1:p1"), "caller"),
    ] {
        let shell = Fake::new();
        lead_panes(&shell, terminal, status);
        let caller = crate::close::Caller {
            pane: caller.map(str::to_owned),
            ..crate::close::Caller::default()
        };
        let error = crate::close::lead(&shell, &ledger, &saved, &caller).unwrap_err();
        assert!(error.to_string().contains(expected), "{expected}: {error}");
        assert!(!calls(&shell).contains(&"herdr pane close w1:p1".to_owned()));
        let record = lead_store::get(&ledger, &saved.id).unwrap().unwrap();
        assert_eq!(record.closed_ms, None);
    }
    // An unknown terminal never authorizes the close, by name or by pane.
    let shell = Fake::new();
    shell
        .on(
            "herdr pane list",
            r#"{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","agent":"pi","agent_status":"idle"}]}}"#,
        )
        .on("herdr workspace list", r#"{"result":{"workspaces":[]}}"#);
    let error = crate::close::local(
        &shell,
        &ledger,
        "w1:p1",
        &crate::close::Caller::default(),
        crate::close::Guard::Clean,
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not prove"), "{error}");
    assert!(!calls(&shell).iter().any(|call| call.contains("close")));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}

#[test]
fn lead_cannot_signal_or_handoff() {
    let ledger = ledger();
    opened(&ledger);
    let shell = Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[
            {"pane_id":"w1:p1","agent":"pi","agent_status":"working",
             "terminal_id":"lead-terminal","agent_session":{"kind":"id","value":"session"}}
        ]}})
        .to_string(),
    );
    let error = crate::done::mark(&ledger, &shell, "w1:p1", 300).unwrap_err();
    assert!(error.to_string().contains("leads have no done"), "{error}");
    for kind in [
        crate::lifecycle::Signal::Ask,
        crate::lifecycle::Signal::Blocked,
    ] {
        let error = crate::done::signal(&ledger, &shell, "w1:p1", kind, "help", 300).unwrap_err();
        assert!(error.to_string().contains("is a lead"), "{error}");
    }
    assert_eq!(crate::done::list(&ledger).unwrap(), Vec::new());
    // An idle lead is never done, so nothing hands it off or closes it.
    let mut agents = vec![crate::probe::Agent {
        pane: "w1:p1".into(),
        state: "done".into(),
        terminal_id: Some("lead-terminal".into()),
        agent_session: Some("session".into()),
        ..crate::probe::Agent::default()
    }];
    crate::done::observe(&ledger, &mut agents).unwrap();
    assert!(!agents[0].done && !agents[0].finishing);
    assert_eq!(agents[0].handoff.as_deref(), Some("none"));
    assert_eq!(
        crate::done::finished(&ledger, &agents).unwrap(),
        std::collections::BTreeSet::new()
    );
    assert!(!calls(&shell).iter().any(|call| call.contains("close")));
    std::fs::remove_dir_all(ledger.dir).unwrap();
}
