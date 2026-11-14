use super::*;
use crate::observation::{Domains, Publisher, Sources};
use portal_types::models::{ModelChoice, ThinkingLevel};
use serde_json::json;

pub(crate) fn selection() -> AgentSettings {
    let model = ModelRef {
        provider: "provider".into(),
        id: "model".into(),
    };
    AgentSettings {
        model: Some(model.clone()),
        thinking: ThinkingLevel::High,
        models: vec![ModelChoice {
            model,
            name: Some("Test model".into()),
            thinking: vec![ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::High],
        }],
    }
}

pub(crate) struct Fixture {
    pub ledger: crate::ledger::Ledger,
    pub publication: Publication,
    pub client: crate::observation::ipc::Client,
    server: Option<crate::observation::ipc::Server>,
    publisher: Publisher,
    input: crate::observation::Publication,
    directory: std::path::PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let directory = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"id\":\"session-a\"}\n").unwrap();
        let shell = crate::shell::Fake::new();
        shell.on("herdr agent list", &json!({"result":{"agents":[{"pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a","agent_session":{"kind":"path","value":path}}]}}).to_string());
        let agents = crate::probe::agent_list(&shell).unwrap();
        let ledger = crate::ledger::Ledger::new(directory.join("state"));
        let mut publisher = Publisher::new().unwrap();
        let input = crate::observation::Publication {
            published_ms: crate::ledger::now_ms(),
            sources: Sources {
                local: BTreeMap::from([("terminals".into(), crate::ledger::now_ms())]),
                ..Sources::default()
            },
            local: Domains::from([("terminals".into(), json!({"agents":agents,"panes":[]}))]),
            views: Domains::new(),
            gaps: vec![],
        };
        publisher.replace(input.clone()).unwrap();
        let mut server = crate::observation::ipc::Server::new(&ledger.dir).unwrap();
        server.bind(publisher.subscribe()).unwrap();
        let client = crate::observation::ipc::Client::at(&ledger.dir);
        let publication = Publication {
            evidence: ProducerEvidence {
                session_file: path.to_string_lossy().into_owned(),
                session_id: "session-a".into(),
            },
            settings: selection(),
        };
        Self {
            ledger,
            publication,
            client,
            server: Some(server),
            publisher,
            input,
            directory,
        }
    }
    pub fn publish(&self) {
        self.client
            .settings("w1:p1", Command::Publish(self.publication.clone()))
            .unwrap();
    }
    fn change_session(&mut self) {
        self.publisher
            .replace(crate::observation::Publication {
                published_ms: crate::ledger::now_ms(),
                sources: Sources {
                    local: BTreeMap::from([("terminals".into(), crate::ledger::now_ms())]),
                    ..Sources::default()
                },
                local: Domains::from([("terminals".into(), json!({"agents":[],"panes":[]}))]),
                views: Domains::new(),
                gaps: vec![],
            })
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.take();
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn settings_observe_skips_publication_reads_until_a_session_publishes() {
    let authority = Authority::default();
    let reads = std::cell::Cell::new(0);
    authority.observe(|| {
        reads.set(reads.get() + 1);
        None
    });
    assert_eq!(reads.get(), 0);
    assert!(authority.state().unwrap().bindings.is_empty());
    let session = Session {
        terminal: None,
        session: "native".into(),
        harness: Some("pi".into()),
        transcript: None,
    };
    authority.state().unwrap().entries.insert(
        "w1:p1".into(),
        Entry {
            session,
            publication: Publication {
                evidence: ProducerEvidence {
                    session_file: "/session".into(),
                    session_id: "session-a".into(),
                },
                settings: selection(),
            },
            requests: None,
        },
    );
    // A missing or stale terminal measurement proves nothing about the session: keep it.
    let measured = |measured_ms: u64| crate::observation::Published {
        version: 1,
        epoch: "epoch".into(),
        revision: 1,
        published_ms: measured_ms,
        local_revision: 1,
        sources: Sources {
            local: BTreeMap::from([("terminals".into(), measured_ms)]),
            ..Sources::default()
        },
        local: Domains::from([("terminals".into(), json!({"agents":[],"panes":[]}))]),
        views: Domains::new(),
        gaps: vec![],
    };
    let stale = measured(crate::ledger::now_ms() - super::super::live_activity::NATIVE_TTL_MS - 1);
    for publication in [None, Some(&stale)] {
        authority.observe(|| {
            reads.set(reads.get() + 1);
            publication
        });
        assert_eq!(authority.state().unwrap().entries.len(), 1);
    }
    assert_eq!(reads.get(), 2);
    // A fresh measurement without the session removes it, and idle observation stops reading.
    let fresh = measured(crate::ledger::now_ms());
    authority.observe(|| {
        reads.set(reads.get() + 1);
        Some(&fresh)
    });
    assert_eq!(reads.get(), 3);
    assert!(authority.state().unwrap().entries.is_empty());
    authority.observe(|| {
        reads.set(reads.get() + 1);
        None
    });
    assert_eq!(reads.get(), 3);
}

#[test]
fn settings_ipc_publish_current_apply_and_refuse() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .client
            .settings("w1:p1", Command::Current)
            .unwrap_err()
            .code,
        "settings_unavailable"
    );
    fixture.publish();
    assert_eq!(
        fixture.client.settings("w1:p1", Command::Current).unwrap(),
        selection()
    );
    let mut subscription = fixture
        .client
        .settings_subscribe("w1:p1", fixture.publication.evidence.clone())
        .unwrap();
    for refusal in [false, true] {
        let client = fixture.client.clone();
        let task = std::thread::spawn(move || {
            client.settings(
                "w1:p1",
                Command::Change(SettingsChange {
                    model: None,
                    thinking: Some(ThinkingLevel::Low),
                }),
            )
        });
        let request = loop {
            if let Some(request) = subscription.next().unwrap() {
                break request;
            }
        };
        let mut settings = selection();
        settings.thinking = ThinkingLevel::Low;
        fixture
            .client
            .settings(
                "w1:p1",
                Command::Result {
                    id: request.id,
                    result: Applied {
                        evidence: request.evidence,
                        settings: (!refusal).then_some(settings.clone()),
                        error: refusal.then(|| "no credentials for provider".into()),
                    },
                },
            )
            .unwrap();
        let outcome = task.join().unwrap();
        if refusal {
            assert_eq!(outcome.unwrap_err().error, "no credentials for provider");
        } else {
            assert_eq!(outcome.unwrap(), settings);
        }
    }
}

#[test]
fn settings_session_change_invalidates_current_subscription_and_pending_request() {
    let mut fixture = Fixture::new();
    fixture.publish();
    let mut subscription = fixture
        .client
        .settings_subscribe("w1:p1", fixture.publication.evidence.clone())
        .unwrap();
    let client = fixture.client.clone();
    let task = std::thread::spawn(move || {
        client.settings(
            "w1:p1",
            Command::Change(SettingsChange {
                model: None,
                thinking: Some(ThinkingLevel::Low),
            }),
        )
    });
    let request = loop {
        if let Some(request) = subscription.next().unwrap() {
            break request;
        }
    };
    fixture.change_session();
    assert_eq!(task.join().unwrap().unwrap_err().code, "session_changed");
    assert_eq!(
        fixture
            .client
            .settings("w1:p1", Command::Current)
            .unwrap_err()
            .code,
        "settings_unavailable"
    );
    assert!(subscription.next().is_err());
    assert!(
        fixture
            .client
            .settings(
                "w1:p1",
                Command::Result {
                    id: request.id,
                    result: Applied {
                        evidence: request.evidence,
                        settings: Some(selection()),
                        error: None
                    }
                }
            )
            .is_err()
    );
}

#[test]
fn settings_timeout_is_ten_seconds_and_late_results_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.publish();
    let mut subscription = fixture
        .client
        .settings_subscribe("w1:p1", fixture.publication.evidence.clone())
        .unwrap();
    let client = fixture.client.clone();
    let start = std::time::Instant::now();
    let task = std::thread::spawn(move || {
        client.settings(
            "w1:p1",
            Command::Change(SettingsChange {
                model: None,
                thinking: Some(ThinkingLevel::Low),
            }),
        )
    });
    let request = loop {
        if let Some(request) = subscription.next().unwrap() {
            break request;
        }
    };
    std::thread::sleep(Duration::from_secs(5));
    fixture
        .input
        .sources
        .local
        .insert("terminals".into(), crate::ledger::now_ms());
    fixture.publisher.replace(fixture.input.clone()).unwrap();
    assert_eq!(task.join().unwrap().unwrap_err().code, "settings_timeout");
    assert!(start.elapsed() >= WAIT);
    assert!(
        fixture
            .client
            .settings(
                "w1:p1",
                Command::Result {
                    id: request.id,
                    result: Applied {
                        evidence: request.evidence,
                        settings: Some(selection()),
                        error: None
                    }
                }
            )
            .is_err()
    );
}

#[test]
fn settings_validate_bounds_and_native_evidence() {
    let fixture = Fixture::new();
    let mut publication = fixture.publication.clone();
    publication.evidence.session_id = "stale".into();
    assert_eq!(
        fixture
            .client
            .settings("w1:p1", Command::Publish(publication))
            .unwrap_err()
            .code,
        "session_changed"
    );
    assert!(
        validate_change(&SettingsChange {
            model: None,
            thinking: None
        })
        .is_err()
    );
    let mut settings = selection();
    settings.models[0].model.id = "x".repeat(257);
    assert!(validate_settings(&settings).is_err());
    let mut settings = selection();
    settings.models = vec![settings.models[0].clone(); 1001];
    assert!(validate_settings(&settings).is_err());
}
