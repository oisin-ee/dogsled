use super::*;
use portal_types::agents::{ActiveTool, LivePhase, LiveState};
use serde_json::json;

#[derive(Debug)]
struct Native {
    directory: std::path::PathBuf,
    agent: crate::probe::Agent,
}

impl Native {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"id\":\"session-a\"}\n").unwrap();
        let shell = crate::shell::Fake::new();
        shell.on("herdr agent list", &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "agent":"pi", "agent_status":"working", "terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":path}
        }]}}).to_string());
        let agent = crate::probe::agent_list(&shell).unwrap().remove(0);
        Self { directory, agent }
    }

    fn authority(&self) -> Authority {
        let mut authority = Authority::new("epoch-a".into());
        authority.observe(std::slice::from_ref(&self.agent), 1000, true);
        authority.sampled_ms = Some(1000);
        authority
    }

    fn register(&self, id: &str, generation: u64) -> RegisterProducer {
        RegisterProducer {
            version: 1,
            epoch: Some("epoch-a".into()),
            expected_generation: generation,
            producer_id: id.into(),
            evidence: ProducerEvidence {
                session_file: self.agent.transcript.clone().unwrap(),
                session_id: "session-a".into(),
            },
        }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn native_shell(native: &Native) -> crate::shell::Fake {
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "agent":"pi", "agent_status":"working", "terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":native.agent.transcript}
        }]}})
        .to_string(),
    );
    shell
}

use crate::test_support::WAIT_DEADLINE as PROCESS_DEADLINE;

#[test]
fn live_activity_register_acknowledges_while_main_collection_is_blocked() {
    let native = Native::new();
    let (entered, started) = std::sync::mpsc::channel();
    let (finished, collected) = std::sync::mpsc::channel();
    let release = ReleaseSample(Arc::new((
        std::sync::Mutex::new(false),
        std::sync::Condvar::new(),
    )));
    let shell = HeldSample {
        fake: crate::shell::Fake::new(),
        entered,
        release: release.0.clone(),
    };
    let ledger = crate::ledger::Ledger::new(native.directory.join("collection"));
    let collection = std::thread::spawn(move || {
        let policy = crate::config::Policy::default();
        let node = crate::node::Node::new(&shell, &ledger, &policy, "/");
        let _collection =
            super::super::collect::Collector::default().collect(&node, crate::ledger::now_ms());
        finished.send(()).unwrap();
    });
    started.recv_timeout(PROCESS_DEADLINE).unwrap();
    let (commands, feed, _handle) =
        Authority::spawn("epoch-a".into(), native_shell(&native)).unwrap();
    let publisher = super::super::Publisher::new().unwrap();
    let state = native.directory.join("ipc");
    let mut server = super::super::ipc::Server::new(&state).unwrap();
    server.activity(commands, feed);
    server.bind(publisher.subscribe()).unwrap();
    let ack = super::super::ipc::Client::at(&state)
        .activity_command("w1:p1", Command::Register(native.register("producer", 0)))
        .unwrap();
    assert_eq!(ack.producer_id, "producer");
    assert!(matches!(
        collected.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    drop(release);
    collected.recv_timeout(PROCESS_DEADLINE).unwrap();
    collection.join().unwrap();
}

#[tokio::test]
async fn live_activity_publish_reaches_feed_without_a_main_publisher_and_revisions_increase() {
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    let native = Native::new();
    let (commands, mut feed, _handle) =
        Authority::spawn("epoch-a".into(), native_shell(&native)).unwrap();
    let ack = commands
        .send(
            "w1:p1".into(),
            Command::Register(native.register("producer", 0)),
        )
        .await
        .unwrap();
    let mut previous = 0;
    for sequence in 1..=4 {
        commands
            .send("w1:p1".into(), Command::Publish(snapshot(&ack, sequence)))
            .await
            .unwrap();
        let view = tokio::time::timeout_at(deadline, async {
            loop {
                let frame = feed.borrow_and_update().clone();
                if let Some(bytes) = frame.views.get("w1:p1") {
                    let view: ActivityView = serde_json::from_slice(bytes).unwrap();
                    if matches!(view.detail, ActivityDetail::Available { sequence: current, .. } if current == sequence) { break view; }
                }
                feed.changed().await.unwrap();
            }
        }).await.unwrap();
        assert_eq!(view.epoch, "epoch-a");
        assert!(view.revision > previous);
        previous = view.revision;
    }
    let mut close = snapshot(&ack, 5);
    close.payload = ProducerPayload::Closed;
    commands
        .send("w1:p1".into(), Command::Publish(close))
        .await
        .unwrap();
    tokio::time::timeout_at(deadline, async {
        loop {
            let frame = feed.borrow_and_update().clone();
            let view: ActivityView = serde_json::from_slice(&frame.views["w1:p1"]).unwrap();
            if matches!(
                view.detail,
                ActivityDetail::Unavailable {
                    reason: UnavailableReason::ProducerClosed
                }
            ) {
                assert!(view.revision > previous);
                break;
            }
            feed.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}

#[derive(Debug)]
struct HeldSample {
    fake: crate::shell::Fake,
    entered: std::sync::mpsc::Sender<()>,
    release: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}

impl crate::shell::Shell for HeldSample {
    fn run(&self, program: &str, args: &[String]) -> crate::error::Result<crate::shell::Output> {
        let _ = self.entered.send(());
        let (lock, ready) = &*self.release;
        let released = lock.lock().unwrap();
        let _released = ready.wait_while(released, |released| !*released).unwrap();
        self.fake.run(program, args)
    }
}

struct ReleaseSample(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
impl Drop for ReleaseSample {
    fn drop(&mut self) {
        let (lock, ready) = &*self.0;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_activity_shutdown_refuses_inflight_pending_and_new_commands() {
    use std::future::Future as _;
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    let native = Native::new();
    let (entered, started) = std::sync::mpsc::channel();
    let release = ReleaseSample(Arc::new((
        std::sync::Mutex::new(false),
        std::sync::Condvar::new(),
    )));
    let shell = HeldSample {
        fake: native_shell(&native),
        entered,
        release: release.0.clone(),
    };
    let (commands, _feed, handle) = Authority::spawn("epoch-a".into(), shell).unwrap();
    let send = commands.clone();
    let registration = native.register("producer", 0);
    let first_registration = registration.clone();
    let first = tokio::spawn(async move {
        send.send("w1:p1".into(), Command::Register(first_registration))
            .await
    });
    started.recv_timeout(PROCESS_DEADLINE).unwrap();
    // Poll the public command future once to admit it while the first sample is held.
    // Its reply must remain pending until shutdown, without inspecting the owner queue.
    let mut queued =
        std::pin::pin!(commands.send("w1:p1".into(), Command::Register(registration.clone())));
    std::future::poll_fn(|context| {
        assert!(queued.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let (finished, stopped) = std::sync::mpsc::channel();
    let joined = std::thread::spawn(move || {
        drop(handle);
        finished.send(()).unwrap();
    });
    let refusal = tokio::time::timeout_at(deadline, first)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code, ActivityCode::UpstreamUnavailable);
    let refusal = tokio::time::timeout_at(deadline, queued)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(refusal.code, ActivityCode::UpstreamUnavailable);
    let refusal = tokio::time::timeout_at(
        deadline,
        commands.send("w1:p1".into(), Command::Register(registration)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(refusal.code, ActivityCode::UpstreamUnavailable);
    drop(release);
    stopped.recv_timeout(PROCESS_DEADLINE).unwrap();
    joined.join().unwrap();
}

fn snapshot(ack: &ActivityAck, sequence: u64) -> PublishActivity {
    PublishActivity {
        version: 1,
        epoch: ack.epoch.clone(),
        generation: ack.generation,
        producer_id: ack.producer_id.clone(),
        session: ack.session.clone(),
        sequence,
        payload: ProducerPayload::Snapshot {
            state: LiveState {
                reasoning: None,
                phase: LivePhase::Thinking,
                active_tools: vec![],
            },
        },
    }
}

fn reason(authority: &Authority, now: Instant, now_ms: u64) -> ActivityDetail {
    authority.views(now, now_ms).remove("w1:p1").unwrap().detail
}

#[test]
fn replacements_fence_duplicates_conflicts_and_delayed_closure() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let registration = native.register("producer-a", 0);
    let first = authority
        .register("w1:p1", registration.clone(), now, 1000)
        .unwrap();
    let update = snapshot(&first, 3);
    assert_eq!(
        authority
            .publish("w1:p1", update.clone(), now, 1000)
            .unwrap()
            .sequence,
        3
    );
    assert_eq!(
        authority
            .publish("w1:p1", update.clone(), now + Duration::from_secs(9), 1000)
            .unwrap()
            .sequence,
        3
    );
    assert_eq!(
        authority
            .register("w1:p1", registration, now + Duration::from_secs(9), 1000)
            .unwrap(),
        first
    );
    let mut conflict = update;
    conflict.payload = ProducerPayload::Closed;
    assert_eq!(
        authority
            .publish("w1:p1", conflict.clone(), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::SequenceConflict
    );
    conflict.sequence = 2;
    assert_eq!(
        authority
            .publish("w1:p1", conflict.clone(), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::SequenceConflict
    );
    conflict.sequence = 4;
    authority
        .publish("w1:p1", conflict.clone(), now, 1000)
        .unwrap();
    let second = authority
        .register(
            "w1:p1",
            native.register("producer-b", first.generation),
            now,
            1000,
        )
        .unwrap();
    assert!(second.generation > first.generation);
    assert_eq!(
        reason(&authority, now, 1000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::NoSnapshot
        }
    );
    conflict.sequence = 4;
    assert_eq!(
        authority
            .publish("w1:p1", conflict, now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    let mut close = snapshot(&second, 1);
    close.payload = ProducerPayload::Closed;
    authority
        .publish("w1:p1", close.clone(), now, 1000)
        .unwrap();
    authority
        .publish("w1:p1", close, now + Duration::from_secs(1), 1000)
        .unwrap();
    assert_eq!(
        authority
            .publish("w1:p1", snapshot(&second, 2), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::ProducerClosed
    );
}

#[test]
fn fresh_takeover_and_generation_zero_probe_are_refused_but_keep_interest() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let first = authority
        .register("w1:p1", native.register("producer-a", 0), now, 1000)
        .unwrap();
    let mut bootstrap = native.register("producer-b", 0);
    bootstrap.epoch = None;
    authority.registration_interest = None;
    assert_eq!(
        authority
            .register("w1:p1", bootstrap, now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    assert_eq!(authority.registration_interest, Some(now));
    assert_eq!(authority.producers["w1:p1"].ack, first);
    let takeover = native.register("producer-b", first.generation);
    assert_eq!(
        authority
            .register("w1:p1", takeover.clone(), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    // Discovery at expiry is not permission if the old owner renews before POST.
    assert_eq!(
        reason(&authority, now + LEASE, 1000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::Expired
        }
    );
    authority
        .publish("w1:p1", snapshot(&first, 1), now + LEASE, 1000)
        .unwrap();
    assert_eq!(
        authority
            .register("w1:p1", takeover, now + LEASE, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    assert_eq!(authority.producers["w1:p1"].ack.producer_id, "producer-a");
    assert_eq!(authority.producers["w1:p1"].ack.sequence, 1);
}

#[test]
fn expired_takeover_fences_delayed_old_updates_and_close() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let first = authority
        .register("w1:p1", native.register("old", 0), now, 1000)
        .unwrap();
    let second = authority
        .register(
            "w1:p1",
            native.register("new", first.generation),
            now + LEASE,
            1000,
        )
        .unwrap();
    assert!(second.generation > first.generation);
    for payload in [snapshot(&first, 1).payload, ProducerPayload::Closed] {
        let mut delayed = snapshot(&first, 2);
        delayed.payload = payload;
        assert_eq!(
            authority
                .publish("w1:p1", delayed, now + LEASE, 1000)
                .unwrap_err()
                .code,
            ActivityCode::StaleGeneration
        );
    }
    assert_eq!(authority.producers["w1:p1"].ack, second);
}

#[tokio::test]
async fn timed_out_queued_registration_commits_and_exact_replay_does_not_renew() {
    let native = Native::new();
    let mut authority = native.authority();
    let commands = authority.commands();
    let mut registration = native.register("queued", 0);
    registration.epoch = None;
    let refusal = tokio::time::timeout(
        Duration::from_secs(6),
        commands.send("w1:p1".into(), Command::Register(registration.clone())),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(refusal.code, ActivityCode::UpstreamUnavailable);
    assert_eq!(authority.pending.len(), 1);
    let now = Instant::now();
    let input = super::super::Publication {
        published_ms: 1000,
        sources: super::super::Sources::default(),
        local: BTreeMap::new(),
        views: BTreeMap::new(),
        gaps: vec![],
    };
    authority
        .refresh_at(&crate::shell::Fake::new(), &input, now, 1000)
        .unwrap();
    let original = authority.producers["w1:p1"].ack.clone();
    assert_eq!(original.sequence, 0);
    authority
        .publish("w1:p1", snapshot(&original, 1), now, 1000)
        .unwrap();
    assert_eq!(
        authority
            .register("w1:p1", registration, now + Duration::from_secs(14), 1000)
            .unwrap(),
        original
    );
    assert_eq!(authority.producers["w1:p1"].received, now);
    assert_eq!(
        reason(&authority, now + LEASE, 1000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::Expired
        }
    );
}

#[test]
fn native_a_b_a_and_disappearance_reject_old_registration_and_updates() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let registration = native.register("producer-a", 0);
    let first = authority
        .register("w1:p1", registration.clone(), now, 1000)
        .unwrap();
    let mut other = native.agent.clone();
    other.terminal_id = Some("terminal-b".into());
    authority.observe(&[other], 1001, true);
    authority.observe(std::slice::from_ref(&native.agent), 1002, true);
    assert_eq!(
        authority
            .register("w1:p1", registration.clone(), now, 1002)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    assert_eq!(
        authority
            .publish("w1:p1", snapshot(&first, 1), now, 1002)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    let generation = authority.bindings["w1:p1"].generation;
    let second = authority
        .register(
            "w1:p1",
            native.register("producer-b", generation),
            now,
            1002,
        )
        .unwrap();
    authority.observe(&[], 1003, true);
    authority.observe(std::slice::from_ref(&native.agent), 1004, true);
    assert_eq!(
        authority
            .register("w1:p1", registration, now, 1004)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    assert_eq!(
        authority
            .publish("w1:p1", snapshot(&second, 1), now, 1004)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
}

#[test]
fn monotonic_expiry_does_not_settle_and_exact_retries_do_not_renew() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    let update = snapshot(&ack, 1);
    authority
        .publish("w1:p1", update.clone(), now, 1000)
        .unwrap();
    authority
        .publish("w1:p1", update.clone(), now + Duration::from_secs(14), 1000)
        .unwrap();
    assert_eq!(
        reason(&authority, now + LEASE, 1000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::Expired
        }
    );
    assert_eq!(
        authority.views(now + LEASE, 1000)["w1:p1"]
            .coarse_state
            .as_deref(),
        Some("working")
    );
    authority
        .publish("w1:p1", snapshot(&ack, 2), now + LEASE, 1000)
        .unwrap();
    assert!(matches!(
        reason(&authority, now + LEASE, 1000),
        ActivityDetail::Available { .. }
    ));
    assert_eq!(
        reason(&authority, now + LEASE, 11000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::NativeUnavailable
        }
    );
}

#[test]
fn producer_interest_expires_without_destroying_current_registration() {
    let native = Native::new();
    let mut authority = native.authority();
    let old = Instant::now().checked_sub(LEASE).unwrap();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), old, 1000)
        .unwrap();
    assert!(authority.next_wake().is_none());
    authority
        .publish("w1:p1", snapshot(&ack, 1), Instant::now(), 1000)
        .unwrap();
    assert!(authority.next_wake().is_some());
}

#[test]
fn restart_and_unreadable_or_changed_header_fail_closed() {
    let native = Native::new();
    let now = Instant::now();
    let mut authority = native.authority();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    let mut restarted = native.authority();
    restarted.epoch = "epoch-b".into();
    assert_eq!(
        restarted
            .publish("w1:p1", snapshot(&ack, 1), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::SessionChanged
    );
    assert_eq!(
        restarted
            .register("w1:p1", native.register("producer", 0), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    std::fs::write(
        native.agent.transcript.as_ref().unwrap(),
        "{\"type\":\"session\",\"id\":\"different\"}\n",
    )
    .unwrap();
    authority.observe(std::slice::from_ref(&native.agent), 1001, true);
    assert!(!authority.producers.contains_key("w1:p1"));
    assert_eq!(
        authority
            .register("w1:p1", native.register("producer", 0), now, 1001)
            .unwrap_err()
            .code,
        ActivityCode::SessionChanged
    );
    std::fs::remove_file(native.agent.transcript.as_ref().unwrap()).unwrap();
    authority.observe(std::slice::from_ref(&native.agent), 1002, true);
    assert_eq!(
        reason(&authority, now, 1002),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::SessionUnverified
        }
    );
}

#[test]
fn native_freshness_uses_source_time_not_publication_time() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    let input = super::super::Publication {
        published_ms: 11000,
        sources: super::super::Sources {
            local: BTreeMap::from([("terminals".into(), 1000)]),
            peers: BTreeMap::new(),
        },
        local: BTreeMap::from([(
            "terminals".into(),
            json!({"agents":[native.agent],"panes":[]}),
        )]),
        views: BTreeMap::new(),
        gaps: vec![],
    };
    authority
        .refresh_at(&crate::shell::Fake::new(), &input, now, input.published_ms)
        .unwrap();
    assert_eq!(
        reason(&authority, now, 11000),
        ActivityDetail::Unavailable {
            reason: UnavailableReason::NativeUnavailable
        }
    );
}

#[test]
fn capacity_never_evicts_an_active_producer() {
    let native = Native::new();
    let mut authority = native.authority();
    let agents: Vec<_> = (0..=CAPACITY)
        .map(|index| {
            let mut agent = native.agent.clone();
            agent.pane = format!("pane-{index}");
            agent
        })
        .collect();
    authority.observe(&agents, 1000, true);
    let now = Instant::now();
    for index in 0..CAPACITY {
        authority
            .register(
                &format!("pane-{index}"),
                native.register(&format!("producer-{index}"), 0),
                now,
                1000,
            )
            .unwrap();
    }
    assert_eq!(
        authority
            .register("pane-128", native.register("extra", 0), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::Capacity
    );
    assert_eq!(authority.producers.len(), CAPACITY);
    authority
        .register("pane-128", native.register("extra", 0), now + LEASE, 1000)
        .unwrap();
    assert_eq!(authority.producers.len(), CAPACITY);
}

#[test]
fn invalid_counters_states_and_unknown_fields_are_rejected() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    let mut body = snapshot(&ack, 0);
    assert_eq!(
        validate_publication(&body).unwrap_err().code,
        ActivityCode::InvalidRequest
    );
    body.sequence = SAFE_COUNTER + 1;
    assert!(validate_publication(&body).is_err());
    body.sequence = 1;
    body.payload = ProducerPayload::Snapshot {
        state: LiveState {
            reasoning: None,
            phase: LivePhase::ToolRunning,
            active_tools: vec![],
        },
    };
    assert!(validate_publication(&body).is_err());
    let tool = ActiveTool {
        call: None,
        call_id: "one".into(),
        name: "read".into(),
        parent_call_id: None,
    };
    body.payload = ProducerPayload::Snapshot {
        state: LiveState {
            reasoning: None,
            phase: LivePhase::ToolRunning,
            active_tools: vec![tool.clone(), tool],
        },
    };
    assert!(validate_publication(&body).is_err());
    let mut wire = serde_json::to_value(snapshot(&ack, 1)).unwrap();
    wire["payload"]["state"]["unknown"] = json!("unrecognized");
    assert!(serde_json::from_value::<PublishActivity>(wire).is_err());
    body.version = 2;
    assert_eq!(
        validate_publication(&body).unwrap_err().code,
        ActivityCode::UnsupportedVersion
    );
}

#[test]
fn reasoning_and_call_lines_accept_the_exact_byte_bounds_and_reject_overflow() {
    let native = Native::new();
    let mut authority = native.authority();
    let ack = authority
        .register(
            "w1:p1",
            native.register("producer", 0),
            Instant::now(),
            1000,
        )
        .unwrap();
    let mut body = snapshot(&ack, 1);
    let ProducerPayload::Snapshot { state } = &mut body.payload else {
        panic!("snapshot")
    };
    state.reasoning = Some("界".repeat(5461) + "!");
    assert!(validate_publication(&body).is_ok());
    let ProducerPayload::Snapshot { state } = &mut body.payload else {
        panic!("snapshot")
    };
    state.reasoning.as_mut().unwrap().push('!');
    assert_eq!(
        validate_publication(&body).unwrap_err().code,
        ActivityCode::InvalidRequest
    );
    let ProducerPayload::Snapshot { state } = &mut body.payload else {
        panic!("snapshot")
    };
    state.reasoning = None;
    state.phase = LivePhase::ToolRunning;
    state.active_tools.push(ActiveTool {
        call_id: "call".into(),
        name: "bash".into(),
        parent_call_id: None,
        call: Some("界".repeat(170) + "\n!"),
    });
    assert!(validate_publication(&body).is_ok());
    let ProducerPayload::Snapshot { state } = &mut body.payload else {
        panic!("snapshot")
    };
    state.active_tools[0].call.as_mut().unwrap().push('!');
    assert_eq!(
        validate_publication(&body).unwrap_err().code,
        ActivityCode::InvalidRequest
    );
}

/// Real IPC forwarding with an independently running activity owner.
#[derive(Debug)]
pub(crate) struct Fixture {
    pub ledger: crate::ledger::Ledger,
    pub registration: RegisterProducer,
    pub client: super::super::ipc::Client,
    server: Option<super::super::ipc::Server>,
    authority: Option<Handle>,
    _publisher: super::super::Publisher,
    native: Native,
}

impl Fixture {
    pub fn new() -> Self {
        let native = Native::new();
        let ledger = crate::ledger::Ledger::new(native.directory.join("state"));
        let mut publisher = super::super::Publisher::new().unwrap();
        let mut registration = native.register("producer", 0);
        registration.epoch = Some(publisher.epoch().into());
        let shell = crate::shell::Fake::new();
        shell.on("herdr agent list", &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "agent":"pi", "agent_status":"working", "terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":native.agent.transcript}
        }]}}).to_string());
        let (commands, feed, authority) =
            Authority::spawn(publisher.epoch().into(), shell).unwrap();
        let mut server = super::super::ipc::Server::new(&ledger.dir).unwrap();
        server.activity(commands, feed);
        let mut details = super::super::details::Collector::default();
        server.details(details.interests.clone());
        publisher.details(details.payloads.clone());
        server.bind(publisher.subscribe()).unwrap();
        let (wake, _) = std::sync::mpsc::channel();
        let dirty = crate::service::wake::Dirty::new(wake);
        let mut input = empty_publication();
        details.refresh(&mut input, &dirty).unwrap();
        assert_eq!(
            details.transcript_reads(),
            0,
            "producer interest must not start transcript workers"
        );
        publisher.replace(input).unwrap();
        // Establish the binding before HTTP tests open a viewer. No service loop runs.
        let client = super::super::ipc::Client::at(&ledger.dir);
        client
            .activity_command("w1:p1", Command::Register(registration.clone()))
            .unwrap();
        Self {
            ledger,
            registration,
            client,
            server: Some(server),
            authority: Some(authority),
            _publisher: publisher,
            native,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.take();
        self.authority.take();
        assert!(self.native.directory.exists());
    }
}

#[test]
fn producer_commands_and_extra_activity_viewers_never_capture_transcripts() {
    let fixture = Fixture::new();
    let ack = fixture
        .client
        .activity_command("w1:p1", Command::Register(fixture.registration.clone()))
        .unwrap();
    let viewers: Vec<_> = (0..8)
        .map(|_| fixture.client.activity_subscribe("w1:p1").unwrap())
        .collect();
    for sequence in 1..=8 {
        fixture
            .client
            .activity_command("w1:p1", Command::Publish(snapshot(&ack, sequence)))
            .unwrap();
    }
    let mut viewer = fixture.client.activity_subscribe("w1:p1").unwrap();
    let deadline = Instant::now() + crate::test_support::WAIT_DEADLINE;
    loop {
        if matches!(
            viewer.current().publication.detail,
            ActivityDetail::Available { sequence: 8, .. }
        ) {
            break;
        }
        assert!(Instant::now() < deadline);
        viewer.changed(Duration::from_millis(50));
    }
    assert!(fixture.client.current().unwrap().local.is_empty());
    drop(viewers);
}

#[tokio::test]
async fn bounded_command_queue_refuses_exhaustion_without_eviction() {
    let mut authority = Authority::new("epoch".into());
    let commands = authority.commands();
    let native = Native::new();
    let mut replies = Vec::new();
    for _ in 0..CAPACITY {
        let (reply, response) = oneshot::channel();
        commands
            .0
            .try_send(Pending {
                pane: "w1:p1".into(),
                command: Command::Register(native.register("producer", 0)),
                reply,
            })
            .unwrap();
        replies.push(response);
    }
    let error = commands
        .send(
            "w1:p1".into(),
            Command::Register(native.register("producer", 0)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ActivityCode::Capacity);
    assert_eq!(authority.pending.len(), CAPACITY);
    authority.pending.close();
}

#[test]
fn older_general_samples_cannot_undo_a_newer_producer_binding_sample() {
    let native = Native::new();
    let mut authority = native.authority();
    let mut newer = native.agent.clone();
    newer.terminal_id = Some("terminal-b".into());
    authority.observe(std::slice::from_ref(&newer), 2000, true);
    authority.sampled_ms = Some(2000);
    let input = super::super::Publication {
        published_ms: 3000,
        sources: super::super::Sources {
            local: BTreeMap::from([("terminals".into(), 1000)]),
            peers: BTreeMap::new(),
        },
        local: BTreeMap::from([(
            "terminals".into(),
            json!({"agents":[native.agent],"panes":[]}),
        )]),
        views: BTreeMap::new(),
        gaps: vec![],
    };
    authority
        .refresh_at(&crate::shell::Fake::new(), &input, Instant::now(), 3000)
        .unwrap();
    assert_eq!(authority.bindings["w1:p1"].session, Session::of(&newer));
}

fn empty_publication() -> super::super::Publication {
    super::super::Publication {
        published_ms: 1000,
        sources: super::super::Sources::default(),
        local: BTreeMap::new(),
        views: BTreeMap::new(),
        gaps: vec![],
    }
}

fn enqueue(authority: &Authority, pane: &str, command: Command) -> oneshot::Receiver<Outcome> {
    let (reply, response) = oneshot::channel();
    authority
        .commands()
        .0
        .try_send(Pending {
            pane: pane.into(),
            command,
            reply,
        })
        .unwrap();
    response
}

#[test]
fn post_receipt_sample_registers_new_pane_and_unverifiable_cached_bindings() {
    for missing in ["cold", "pane", "stale", "session", "evidence"] {
        let native = Native::new();
        let shell = crate::shell::Fake::new();
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a",
                "agent_session":{"kind":"path","value":native.agent.transcript}
            }]}})
            .to_string(),
        );
        let mut authority = native.authority();
        match missing {
            "cold" => authority.sampled_ms = None,
            "pane" => authority.observe(&[], 1000, true),
            "session" => authority.bindings.get_mut("w1:p1").unwrap().session = None,
            "evidence" => {
                authority.bindings.get_mut("w1:p1").unwrap().evidence = Some(ProducerEvidence {
                    session_file: "old-session.jsonl".into(),
                    session_id: "old-session".into(),
                });
            }
            _ => {}
        }
        let now_ms = if missing == "stale" { 11000 } else { 1001 };
        let mut response = enqueue(
            &authority,
            "w1:p1",
            Command::Register(native.register(
                "startup",
                authority.generation + u64::from(missing == "session"),
            )),
        );
        authority
            .refresh_at(&shell, &empty_publication(), Instant::now(), now_ms)
            .unwrap();
        let ack = response.try_recv().unwrap().unwrap();
        assert_eq!(ack.pane, "w1:p1", "{missing}");
        assert_eq!(ack.sequence, 0, "{missing}");
        assert_eq!(authority.sampled_ms, Some(now_ms), "{missing}");
        assert_eq!(authority.bindings["w1:p1"].measured_ms, now_ms, "{missing}");
        assert_eq!(
            *shell.calls.lock().unwrap(),
            vec!["herdr agent list"],
            "{missing}"
        );
    }
}

#[test]
fn post_receipt_sample_verifies_session_file_created_after_last_sample() {
    let native = Native::new();
    let path = native.agent.transcript.as_ref().unwrap();
    std::fs::remove_file(path).unwrap();
    let mut authority = native.authority();
    assert!(authority.bindings["w1:p1"].evidence.is_none());
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":path}
        }]}})
        .to_string(),
    );
    let mut response = enqueue(
        &authority,
        "w1:p1",
        Command::Register(native.register("startup", 0)),
    );
    std::fs::write(path, "{\"type\":\"session\",\"id\":\"session-a\"}\n").unwrap();
    authority
        .refresh_at(&shell, &empty_publication(), Instant::now(), 1001)
        .unwrap();
    assert_eq!(response.try_recv().unwrap().unwrap().sequence, 0);
    assert_eq!(
        authority.bindings["w1:p1"].evidence,
        Some(native.register("startup", 0).evidence)
    );
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
}

#[test]
fn post_receipt_sample_confirms_agent_missing_before_refusing_registration() {
    let native = Native::new();
    let mut authority = Authority::new("epoch-a".into());
    authority.observe(&[], 1000, true);
    authority.sampled_ms = Some(1000);
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[]}}).to_string(),
    );
    for now_ms in [1001, 1002] {
        let mut response = enqueue(
            &authority,
            "w1:p1",
            Command::Register(native.register("startup", 0)),
        );
        authority
            .refresh_at(&shell, &empty_publication(), Instant::now(), now_ms)
            .unwrap();
        assert_eq!(
            response.try_recv().unwrap().unwrap_err().code,
            ActivityCode::AgentMissing
        );
        assert_eq!(authority.sampled_ms, Some(now_ms));
        assert_eq!(
            authority.registration_refusals["w1:p1"],
            ActivityCode::AgentMissing
        );
    }
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"; 2]);
    // Once registration interest lapses, the unbound pane's refusal is forgotten.
    let lapsed = Instant::now() + Duration::from_secs(16);
    authority
        .refresh_at(&shell, &empty_publication(), lapsed, 1003)
        .unwrap();
    assert!(authority.registration_refusals.is_empty());
    // An invalid pane name is refused without being recorded or logged.
    let mut response = enqueue(
        &authority,
        "w1:p1\nforged log line",
        Command::Register(native.register("startup", 0)),
    );
    authority
        .refresh_at(&shell, &empty_publication(), lapsed, 1004)
        .unwrap();
    assert_eq!(
        response.try_recv().unwrap().unwrap_err().code,
        ActivityCode::InvalidRequest
    );
    assert!(authority.registration_refusals.is_empty());
}

#[test]
fn queued_publish_heartbeats_and_views_never_force_native_samples() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    let shell = crate::shell::Fake::new();
    let input = empty_publication();
    for now_ms in [1001, 11000] {
        for sequence in 1..=3 {
            let mut response = enqueue(
                &authority,
                "w1:p1",
                Command::Publish(snapshot(&ack, sequence)),
            );
            authority.refresh_at(&shell, &input, now, now_ms).unwrap();
            let result = response.try_recv().unwrap();
            if now_ms == 1001 {
                assert_eq!(result.unwrap().sequence, sequence);
            } else {
                assert_eq!(result.unwrap_err().code, ActivityCode::UpstreamUnavailable);
            }
            for _ in 0..8 {
                assert_eq!(authority.views(now, now_ms).len(), 1);
            }
        }
    }
    assert!(shell.calls.lock().unwrap().is_empty());
    assert_eq!(authority.sampled_ms, Some(1000));
}

#[test]
fn three_pending_registrations_share_one_post_receipt_sample() {
    let native = Native::new();
    let mut authority = Authority::new("epoch-a".into());
    let shell = crate::shell::Fake::new();
    let panes = ["w1:p1", "w1:p2", "w1:p3"];
    let agents: Vec<_> = panes
        .iter()
        .map(|pane| {
            json!({
                "pane_id":pane,"agent":"pi","agent_status":"working","terminal_id":"terminal-a",
                "agent_session":{"kind":"path","value":native.agent.transcript}
            })
        })
        .collect();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":agents}}).to_string(),
    );
    let mut responses: Vec<_> = panes
        .iter()
        .map(|pane| {
            enqueue(
                &authority,
                pane,
                Command::Register(native.register(pane, 0)),
            )
        })
        .collect();
    authority
        .refresh_at(&shell, &empty_publication(), Instant::now(), 1001)
        .unwrap();
    for (pane, response) in panes.iter().zip(&mut responses) {
        let ack = response.try_recv().unwrap().unwrap();
        assert_eq!(&ack.pane, pane);
        assert_eq!(ack.sequence, 0);
    }
    assert_eq!(authority.producers.len(), 3);
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
}

#[test]
fn registration_refusal_code_changes_clear_on_success_and_pane_departure() {
    let native = Native::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let mut invalid = native.register("producer", 0);
    invalid.version = 2;
    for code in [
        ActivityCode::UnsupportedVersion,
        ActivityCode::UnsupportedVersion,
    ] {
        assert_eq!(
            authority
                .apply("w1:p1", Command::Register(invalid.clone()), now, 1000)
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(authority.registration_refusals["w1:p1"], code);
    }
    invalid.version = 1;
    invalid.expected_generation = 1;
    assert_eq!(
        authority
            .apply("w1:p1", Command::Register(invalid), now, 1000)
            .unwrap_err()
            .code,
        ActivityCode::StaleGeneration
    );
    assert_eq!(
        authority.registration_refusals["w1:p1"],
        ActivityCode::StaleGeneration
    );
    authority
        .apply(
            "w1:p1",
            Command::Register(native.register("producer", 0)),
            now,
            1000,
        )
        .unwrap();
    assert!(authority.registration_refusals.is_empty());
    authority
        .apply(
            "w1:p1",
            Command::Register(native.register("other", 0)),
            now,
            1000,
        )
        .unwrap_err();
    assert_eq!(authority.registration_refusals.len(), 1);
    authority.observe(&[], 1001, true);
    assert!(authority.registration_refusals.is_empty());
}

#[test]
fn cold_bootstrap_and_missing_bindings_keep_cheap_sampling_until_recovery() {
    for missing in ["cold", "pane", "evidence", "transition"] {
        let native = Native::new();
        let shell = crate::shell::Fake::new();
        shell.on(
            "herdr agent list",
            &json!({"result":{"agents":[{
                "pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a",
                "agent_session":{"kind":"path","value":native.agent.transcript}
            }]}})
            .to_string(),
        );
        let mut authority = if missing == "cold" {
            Authority::new("epoch-a".into())
        } else {
            native.authority()
        };
        match missing {
            "pane" => authority.observe(&[], 1000, true),
            "evidence" => authority.bindings.get_mut("w1:p1").unwrap().evidence = None,
            "transition" => {
                let mut changed = native.agent.clone();
                changed.terminal_id = Some("terminal-b".into());
                authority.observe(&[changed], 1000, true);
            }
            _ => {}
        }
        let now = Instant::now();
        let mut bootstrap = native.register("startup", 0);
        bootstrap.epoch = None;
        let code = authority
            .register("w1:p1", bootstrap.clone(), now, 1000)
            .unwrap_err()
            .code;
        assert_eq!(
            code,
            match missing {
                "cold" => ActivityCode::UpstreamUnavailable,
                "pane" => ActivityCode::AgentMissing,
                "evidence" => ActivityCode::SessionUnverified,
                _ => ActivityCode::StaleGeneration,
            }
        );
        assert!(
            authority.sample_due_at(now),
            "{missing} must schedule interest without viewers"
        );
        assert!(shell.calls.lock().unwrap().is_empty());
        authority.sample(&shell, 1001);
        let generation = authority.views(Instant::now(), 1001)["w1:p1"]
            .producer
            .generation;
        let registration = if generation == 0 {
            bootstrap
        } else {
            native.register("startup", generation)
        };
        let ack = authority
            .register("w1:p1", registration, Instant::now(), 1001)
            .unwrap();
        assert_eq!(ack.sequence, 0);
        assert!(!authority.sample_due());
        assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
    }
}

#[test]
fn stale_first_registration_schedules_cheap_sampling_and_retry_succeeds() {
    let native = Native::new();
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":native.agent.transcript}
        }]}})
        .to_string(),
    );
    let mut authority = native.authority();
    let now = Instant::now();
    let registration = native.register("first-producer", 0);
    assert!(authority.next_wake_at(now).is_none());
    let refusal = authority
        .register("w1:p1", registration.clone(), now, 11000)
        .unwrap_err();
    assert_eq!(refusal.code, ActivityCode::UpstreamUnavailable);
    assert!(authority.sample_due_at(now));
    assert!(
        shell.calls.lock().unwrap().is_empty(),
        "a registration command must not read native state"
    );
    authority.sample(&shell, 11000);
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
    let ack = authority
        .register("w1:p1", registration, Instant::now(), 11000)
        .unwrap();
    assert_eq!(ack.sequence, 0);
    assert!(!authority.sample_due());
    assert_eq!(
        *shell.calls.lock().unwrap(),
        vec!["herdr agent list"],
        "a retry must not capture a screen or transcript"
    );
}

#[test]
fn stale_registration_interest_is_rate_limited_and_expires_after_last_receipt() {
    let native = Native::new();
    let shell = crate::shell::Fake::new();
    let mut authority = native.authority();
    let now = Instant::now();
    let registration = native.register("first-producer", 0);
    let mut invalid = registration.clone();
    invalid.version = 2;
    assert_eq!(
        authority
            .register("w1:p1", invalid, now, 11000)
            .unwrap_err()
            .code,
        ActivityCode::UnsupportedVersion
    );
    assert!(
        authority.next_wake_at(now).is_none(),
        "unrelated refusals must not create sampling interest"
    );
    assert_eq!(
        authority
            .register("w1:p1", registration.clone(), now, 11000)
            .unwrap_err()
            .code,
        ActivityCode::UpstreamUnavailable
    );
    assert!(authority.sample_due_at(now));
    // The native sample fails, so registrations continue to be refused. They must retain
    // interest without moving the shared five-second sampling deadline forward or back.
    authority.sample(&shell, 11000);
    let deadline = authority.next_wake().unwrap();
    let sampled_at = deadline.checked_sub(SAMPLE).unwrap();
    let mut last_receipt = sampled_at;
    for second in 0..5 {
        last_receipt = sampled_at + Duration::from_secs(second);
        for _ in 0..8 {
            assert_eq!(
                authority
                    .register(
                        "w1:p1",
                        registration.clone(),
                        last_receipt,
                        11000 + second * 1000
                    )
                    .unwrap_err()
                    .code,
                ActivityCode::UpstreamUnavailable
            );
            assert_eq!(authority.next_wake_at(last_receipt), Some(deadline));
            assert!(!authority.sample_due_at(last_receipt));
        }
    }
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
    assert!(authority.sample_due_at(deadline));
    assert!(
        authority
            .next_wake_at(last_receipt + LEASE.saturating_sub(Duration::from_millis(1)))
            .is_some()
    );
    assert!(authority.next_wake_at(last_receipt + LEASE).is_none());
    assert!(!authority.sample_due_at(last_receipt + LEASE));
    assert!(authority.producers.is_empty());
}

#[test]
fn binding_sampling_is_cheap_shared_and_expired_interest_stops_it() {
    let native = Native::new();
    let shell = crate::shell::Fake::new();
    shell.on(
        "herdr agent list",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1","agent":"pi","agent_status":"working","terminal_id":"terminal-a",
            "agent_session":{"kind":"path","value":native.agent.transcript}
        }]}})
        .to_string(),
    );
    let mut authority = native.authority();
    let now = Instant::now();
    let ack = authority
        .register("w1:p1", native.register("producer", 0), now, 1000)
        .unwrap();
    assert!(authority.sample_due());
    authority.sample(&shell, 1001);
    for sequence in 1..=10 {
        authority
            .publish("w1:p1", snapshot(&ack, sequence), now, 1001)
            .unwrap();
        assert!(!authority.sample_due());
    }
    assert_eq!(*shell.calls.lock().unwrap(), vec!["herdr agent list"]);
    authority.producers.get_mut("w1:p1").unwrap().received = now.checked_sub(LEASE).unwrap();
    assert!(
        authority.next_wake().is_some(),
        "registration interest remains live"
    );
    authority.registration_interest = now.checked_sub(LEASE);
    assert!(authority.next_wake().is_none());
}
