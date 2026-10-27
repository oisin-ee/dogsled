//! Temporary portal servers for one interview or one plan review.

use std::io::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use portal_types::questions::{AskBody, Responses};
use portal_types::reviews::{DecisionEvent, SubmitBody};
use tokio::sync::watch;

use crate::cli::Command;
use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::shell::Shell;

use super::{AppState, reviews, store::Poll};

/// Run a one-shot server with the same router and stores as the portal.
pub fn command(shell: &dyn Shell, ledger: &Ledger, command: Command) -> Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(shell, ledger, command))
}

async fn run(shell: &dyn Shell, ledger: &Ledger, command: Command) -> Result<ExitCode> {
    let (_sender, receiver) = watch::channel(super::empty_payload());
    let (listener, state) = super::listen(ledger, 0, receiver).await?;
    let app = super::router(state.clone());
    let mut server = tokio::spawn(async move { axum::serve(listener, app).await });
    let work = async {
        match command {
            Command::Interview {
                file,
                pane,
                session,
                cwd,
                timeout,
            } => {
                let body = AskBody {
                    questions: serde_json::from_slice(&std::fs::read(file)?)?,
                    pane,
                    session,
                    cwd: match cwd {
                        Some(cwd) => cwd,
                        None => std::env::current_dir()?.to_string_lossy().into_owned(),
                    },
                };
                if let Some(answers) =
                    interview(&state, shell, body, Duration::from_secs(timeout)).await?
                {
                    print_json(&answers)?;
                    Ok(ExitCode::SUCCESS)
                } else {
                    eprintln!("the interview timed out or was withdrawn");
                    Ok(ExitCode::from(2))
                }
            }
            Command::Review {
                file,
                pane,
                session,
                cwd,
            } => {
                let file = if file.is_absolute() {
                    file
                } else {
                    std::path::Path::new(&cwd).join(file)
                };
                let body = SubmitBody {
                    path: file.to_string_lossy().into_owned(),
                    content: std::fs::read_to_string(file)?,
                    pane,
                    session,
                    cwd,
                };
                let event = review(&state, shell, body).await?;
                // Flush before acknowledging: a failed stdout write must leave the decision pending.
                print_json(&event)?;
                reviews::acknowledge(&state.web, event.review_id).await?;
                Ok(ExitCode::SUCCESS)
            }
            _ => Err(Error::Invalid("expected interview or review".to_owned())),
        }
    };
    let result = tokio::select! {
        result = work => result,
        result = &mut server => {
            match result {
                Ok(result) => result?,
                Err(error) => return Err(Error::Invalid(format!("the temporary portal failed: {error}"))),
            }
            return Err(Error::Invalid("the temporary portal stopped before an answer".to_owned()));
        }
    };
    server.abort();
    let _ = server.await;
    result
}

/// Create and wait through the question store used by the HTTP routes.
async fn interview(
    state: &AppState,
    shell: &dyn Shell,
    body: AskBody,
    timeout: Duration,
) -> Result<Option<Responses>> {
    let pane = query_value(&body.pane);
    let id = state.questions.create(body).await?;
    let url = format!("http://127.0.0.1:{}/?question={id}&pane={pane}", state.port);
    if let Err(error) = open(shell, &url) {
        state.questions.withdraw(&id).await?;
        return Err(error);
    }
    let answer = tokio::select! {
        answer = state.questions.wait(&id, timeout) => answer?,
        () = super::shutdown() => Poll::Gone,
    };
    match answer {
        Poll::Answered(answers) => Ok(Some(answers)),
        Poll::Timeout | Poll::Gone => {
            state.questions.withdraw(&id).await?;
            Ok(None)
        }
    }
}

/// Submit and wait through the review store used by the HTTP routes.
async fn review(state: &AppState, shell: &dyn Shell, body: SubmitBody) -> Result<DecisionEvent> {
    let session = body.session.clone();
    let submitted = reviews::submit_plan(&state.web, body).await?;
    let url = format!("http://127.0.0.1:{}/?review={}", state.port, submitted.id);
    open(shell, &url)?;
    tokio::select! {
        event = reviews::wait_for_decision(&state.web, session, &submitted.id) => event,
        () = super::shutdown() => Err(Error::Invalid("the review was aborted".to_owned())),
    }
}

/// Open the tab through the operating-system seam. Only the result goes to stdout.
fn open(shell: &dyn Shell, url: &str) -> Result<()> {
    eprintln!("{url}");
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    shell.run_setup_ok(program, &[url.to_owned()])?;
    Ok(())
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

/// Encode an arbitrary pane name as a query value, not URL syntax.
fn query_value(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::Output;
    use std::io::Read as _;
    use tokio::sync::mpsc;

    #[derive(Debug)]
    struct Browser(mpsc::UnboundedSender<String>);

    impl Shell for Browser {
        fn run(&self, program: &str, args: &[String]) -> Result<Output> {
            assert_eq!(
                program,
                if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                }
            );
            let url = args
                .first()
                .ok_or_else(|| Error::Invalid("missing URL".to_owned()))?;
            self.0
                .send(url.clone())
                .map_err(|error| Error::Invalid(error.to_string()))?;
            Ok(Output {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    async fn fixture() -> Result<(Ledger, tokio::net::TcpListener, AppState)> {
        let ledger = Ledger::new(
            std::env::temp_dir().join(format!("dogsled-oneshot-{}", crate::ledger::new_id())),
        );
        let (_sender, receiver) = watch::channel(super::super::empty_payload());
        let (listener, state) = super::super::listen(&ledger, 0, receiver).await?;
        assert_eq!(listener.local_addr()?.port(), state.port);
        assert_ne!(state.port, 0);
        Ok((ledger, listener, state))
    }

    /// Reach the actual listener without introducing a production HTTP client.
    async fn post(port: u16, path: String, body: &'static str, origin: u16) -> Result<String> {
        tokio::task::spawn_blocking(move || -> Result<String> {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            write!(stream, "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            Ok(response)
        }).await.map_err(|error| Error::Invalid(error.to_string()))?
    }

    fn ask() -> Result<AskBody> {
        Ok(serde_json::from_value(serde_json::json!({
            "questions": {"questions": [{"id": "q1", "type": "text", "question": "Which?"}]},
            "pane": "pane & name", "session": "session", "cwd": "/repo"
        }))?)
    }

    #[tokio::test]
    async fn interview_returns_the_answer_posted_to_the_random_port() -> Result<()> {
        let (ledger, listener, state) = fixture().await?;
        let app = super::super::router(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let (send, mut opened) = mpsc::unbounded_channel();
        let browser = Browser(send);
        let work = interview(&state, &browser, ask()?, Duration::from_secs(5));
        let answer = async {
            let url = opened
                .recv()
                .await
                .ok_or_else(|| Error::Invalid("no tab".to_owned()))?;
            assert!(url.ends_with("&pane=pane%20%26%20name"));
            let pending = state.questions.list().await?;
            let question = pending
                .first()
                .ok_or_else(|| Error::Invalid("no question".to_owned()))?;
            let path = format!("/api/questions/{}/answer", question.id);
            // The actual listener port is the only origin allowed to answer.
            let refused = post(state.port, path.clone(), r#"{"responses":[]}"#, 0).await?;
            assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
            let response = post(
                state.port,
                path,
                r#"{"responses":[{"id":"q1","value":"B"}]}"#,
                state.port,
            )
            .await?;
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            Ok::<(), Error>(())
        };
        let (answers, posted) = tokio::join!(work, answer);
        server.abort();
        let _ = server.await;
        posted?;
        let answers = answers?.ok_or_else(|| Error::Invalid("no answer".to_owned()))?;
        assert_eq!(serde_json::to_value(answers)?["responses"][0]["value"], "B");
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn interview_timeout_withdraws_the_question() -> Result<()> {
        let (ledger, _listener, state) = fixture().await?;
        let (send, _opened) = mpsc::unbounded_channel();
        assert!(
            interview(&state, &Browser(send), ask()?, Duration::ZERO)
                .await?
                .is_none()
        );
        assert_eq!(state.questions.list().await?, []);
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn review_returns_and_acknowledges_the_decision_posted_to_the_router() -> Result<()> {
        let (ledger, listener, state) = fixture().await?;
        let app = super::super::router(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let (send, mut opened) = mpsc::unbounded_channel();
        let browser = Browser(send);
        let body = SubmitBody {
            path: "/repo/plan.md".to_owned(),
            content: "# Plan".to_owned(),
            pane: "pane".to_owned(),
            session: "session".to_owned(),
            cwd: "/repo".to_owned(),
        };
        let work = review(&state, &browser, body);
        let decide = async {
            let url = opened
                .recv()
                .await
                .ok_or_else(|| Error::Invalid("no tab".to_owned()))?;
            let (_, id) = url
                .split_once("?review=")
                .ok_or_else(|| Error::Invalid("no review link".to_owned()))?;
            let response = post(
                state.port,
                format!("/api/reviews/{id}/approve"),
                r#"{"notes":"Proceed."}"#,
                state.port,
            )
            .await?;
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            Ok::<(), Error>(())
        };
        let (decision, posted) = tokio::join!(work, decide);
        server.abort();
        let _ = server.await;
        posted?;
        let decision = decision?;
        assert_eq!(decision.message, "Proceed.");
        assert_eq!(decision.plan_content, "# Plan");
        assert_eq!(serde_json::to_value(&decision)?["decision"], "approved");
        assert_eq!(
            reviews::acknowledge(&state.web, decision.review_id).await?,
            Some(true)
        );
        std::fs::remove_dir_all(ledger.dir)?;
        Ok(())
    }
}
