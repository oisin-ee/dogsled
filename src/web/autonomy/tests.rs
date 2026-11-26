use super::*;
use crate::observation::{Domains, Publication, Publisher, Sources};
use portal_types::autonomy::{AutonomyCause, DecisionView};
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};

fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    origin: &str,
    body: &str,
) -> (String, serde_json::Value) {
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(crate::test_support::WAIT_DEADLINE))
        .unwrap();
    write!(socket, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    let body = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(body).unwrap()
    };
    (headers.to_owned(), body)
}

#[tokio::test]
async fn autonomy_routes_read_decisions_and_refuse_a_worker_switch() {
    let ledger = Ledger::temporary();
    let mut publisher = Publisher::new().unwrap();
    let mut observer = ipc::Server::new(&ledger.dir).unwrap();
    observer.bind(publisher.subscribe()).unwrap();
    let decision = json!({"id":"d1","host":"worker-host","pane":"worker",
        "owner_host":crate::inbox::hostname(),"owner_pane":"lead","created_ms":9,"source":"agent",
        "title":"Export CSV","decision":"Use CSV","why":"The reader accepts CSV"});
    let lead_decision = json!({"id":"d2","host":"local","pane":"lead","created_ms":3,"source":"agent",
        "title":"Keep column order","decision":"Keep the order","why":"Readers use positions"});
    let unrelated = json!({"id":"d3","host":"other-host","pane":"lead","created_ms":12,"source":"agent",
        "title":"Use JSON","decision":"Use JSON","why":"This reader accepts JSON"});
    publisher.replace(Publication {
        published_ms: 10,
        sources: Sources::default(),
        local: Domains::from([("host".into(), json!({
            "owned_work":{"version":1,"status":"ready","hostname":crate::inbox::hostname(),"owned":[],"executions":[{
                "kind":"agent","request_id":"r1","owner_hostname":"peer","owner_pane":"its-lead",
                "pane":"local-worker","title":"worker","job_label":null
            }]}
        }))]),
        views: Domains::from([
            ("portal".into(), json!({"data":{
                "version":portal_types::PORTAL_VERSION,"epoch":"test","revision":1,"published_ms":10,
                "board":null,"agents":null,"inbox":null,"repos":null,"gaps":[]
            },"gaps":[]})),
            (autonomy::VIEW.into(), json!({
                "data":{"panes":{"lead":"agent"},"decisions":[lead_decision, unrelated, decision]},
                "gaps":[{"domain":"autonomy:old-host","availability":"unsupported","detail":"autonomous unsupported"}]
            })),
        ]),
        gaps: vec![],
    }).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(
        tokio::sync::watch::channel(super::super::PortalState::default()).1,
        address.port(),
        None,
        super::super::guard::Token::new("secret".into()),
        ledger.clone(),
    );
    let stop = state.stop.clone();
    let server = tokio::spawn(super::super::serve_until(
        listener,
        state,
        stop.clone().cancelled_owned(),
    ));
    let _cancel = stop.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        assert_eq!(
            autonomy::cause_of(&ledger, "lead").unwrap(),
            Some(AutonomyCause::Agent)
        );
        assert_eq!(autonomy::cause_of(&ledger, "missing").unwrap(), None);
        let origin = format!("http://{address}");
        let (headers, body) = request(
            address,
            "GET",
            "/api/agents/local/lead/decisions",
            &origin,
            "",
        );
        assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
        assert!(headers.contains("cache-control: no-store"), "{headers}");
        let list: DecisionList = serde_json::from_value(body).unwrap();
        assert_eq!(
            list.decisions,
            [decision, lead_decision]
                .map(|value| serde_json::from_value::<DecisionView>(value).unwrap())
        );
        assert_eq!(
            list.gaps,
            vec![HostGap {
                host: "old-host".into(),
                error: "autonomous unsupported".into()
            }]
        );
        let path = "/api/agents/local/local-worker/autonomy";
        let (headers, _) = request(
            address,
            "POST",
            path,
            "http://other.example",
            r#"{"on":true}"#,
        );
        assert!(headers.starts_with("HTTP/1.1 403"), "{headers}");
        let (headers, body) = request(address, "POST", path, &origin, r#"{"on":true}"#);
        assert!(headers.starts_with("HTTP/1.1 409"), "{headers}");
        assert_eq!(body["error"], "Workers inherit autonomy from their owner");
        assert_eq!(
            autonomy::agents(&ledger).unwrap(),
            [] as [(String, String); 0]
        );
    })
    .await
    .unwrap();
    stop.cancel();
    server.await.unwrap().unwrap();
}
