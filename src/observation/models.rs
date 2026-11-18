//! Serve-owned Pi model catalogue. Readers consume the retained publication.

use crate::error::{Error, Result};
use crate::shell::Shell;
use portal_types::models::{ModelCatalogue, ModelChoice};
use serde_json::Value;

const HOUR_MS: u64 = 60 * 60 * 1000;
const INPUT: &[u8] = b"{\"id\":\"models\",\"type\":\"get_entries\"}\n";

#[derive(Debug, Default)]
pub(super) struct Catalogue {
    attempted_ms: Option<u64>,
    list: ModelCatalogue,
}

impl Catalogue {
    pub fn refresh(&mut self, shell: &dyn Shell, now: u64) -> Option<ModelCatalogue> {
        if self
            .attempted_ms
            .is_some_and(|previous| now.saturating_sub(previous) < HOUR_MS)
        {
            return None;
        }
        self.attempted_ms = Some(now);
        match read(shell) {
            Ok(models) => {
                self.list = ModelCatalogue {
                    models,
                    error: None,
                }
            }
            Err(error) => self.list.error = Some(error.to_string()),
        }
        Some(self.list.clone())
    }
}

fn read(shell: &dyn Shell) -> Result<Vec<ModelChoice>> {
    let output = shell.run_input_bounded(
        "pi",
        &[
            "--mode".into(),
            "rpc".into(),
            "--no-session".into(),
            "--dogsled-model-catalogue".into(),
        ],
        INPUT,
        shell.budget().with_limit("pi", crate::budget::CALL_LIMIT)?,
    )?;
    if !output.ok() {
        return Err(Error::Invalid(format!(
            "Pi model catalogue: pi exited {}: {}",
            output.status,
            output.stderr.trim()
        )));
    }
    parse(&output.stdout)
}

fn parse(transcript: &str) -> Result<Vec<ModelChoice>> {
    for line in transcript.lines() {
        let Ok(response) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if response["id"] != "models" || response["type"] != "response" {
            continue;
        }
        if response["success"] != true {
            return Err(Error::Invalid("Pi model catalogue: request refused".into()));
        }
        let data = response["data"]["entries"]
            .as_array()
            .and_then(|entries| {
                entries.iter().find(|entry| {
                    entry["type"] == "custom" && entry["customType"] == "dogsled.model-catalogue.v1"
                })
            })
            .and_then(|entry| entry.get("data"))
            .ok_or_else(|| {
                Error::Invalid("Pi model catalogue: missing Dogsled discovery entry".into())
            })?;
        return Ok(serde_json::from_value(data.clone())?);
    }
    Err(Error::Invalid(
        "Pi model catalogue: missing response".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Fake, Output};

    // RPC wire transcript includes extension startup traffic and unrelated responses.
    const TRANSCRIPT: &str = r#"{"type":"extension_ui_request","id":"ui-1","method":"setStatus","text":"ready"}
{"type":"response","id":"other","success":true,"data":{}}
{"type":"response","id":"models","success":true,"data":{"entries":[{"type":"custom","customType":"dogsled.model-catalogue.v1","data":[{"model":{"provider":"p","id":"one"},"scoped":true,"thinking":["high"]},{"model":{"provider":"p","id":"two"},"scoped":false,"thinking":["off"]}]}]}}"#;
    const COMMAND: &str = "pi --mode rpc --no-session --dogsled-model-catalogue";

    #[test]
    fn missing_refused_and_malformed_responses_fail() {
        for transcript in [
            "",
            r#"{"type":"extension_ui_request","id":"models"}"#,
            r#"{"type":"response","id":"models","success":false,"error":"no models"}"#,
            r#"{"type":"response","id":"models","success":true,"data":{}}"#,
            r#"{"type":"response","id":"models","success":true,"data":{"models":[{"type":"chat"}]}}"#,
        ] {
            assert!(parse(transcript).is_err(), "{transcript}");
        }
    }

    #[derive(Debug)]
    struct RpcShell {
        timeout: bool,
    }

    impl Shell for RpcShell {
        fn run(&self, _program: &str, _args: &[String]) -> Result<Output> {
            Err(Error::Invalid("RPC must use bounded stdin".into()))
        }

        fn run_input_bounded(
            &self,
            program: &str,
            args: &[String],
            input: &[u8],
            timeout: std::time::Duration,
        ) -> Result<Output> {
            assert_eq!(program, "pi");
            assert_eq!(
                args,
                ["--mode", "rpc", "--no-session", "--dogsled-model-catalogue"]
            );
            assert_eq!(input, INPUT);
            assert!(timeout <= crate::budget::CALL_LIMIT);
            if self.timeout {
                return Err(Error::Invalid("Pi RPC timed out".into()));
            }
            Ok(Output {
                status: 0,
                stdout: TRANSCRIPT.into(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn rpc_uses_bounded_stdin_and_retains_models_on_timeout() {
        let mut catalogue = Catalogue::default();
        let good = catalogue.refresh(&RpcShell { timeout: false }, 0).unwrap();
        let failed = catalogue
            .refresh(&RpcShell { timeout: true }, HOUR_MS)
            .unwrap();
        assert_eq!(failed.models, good.models);
        assert!(failed.error.as_deref().unwrap().contains("timed out"));
        let cold = Catalogue::default()
            .refresh(&RpcShell { timeout: true }, 0)
            .unwrap();
        assert_eq!(cold.models, []);
        assert!(cold.error.is_some());
    }

    #[test]
    fn catalogue_retains_list_on_failure_and_recovers_hourly() {
        let shell = Fake::new();
        shell.on(COMMAND, TRANSCRIPT);
        let mut catalogue = Catalogue::default();
        let good = catalogue.refresh(&shell, 0).unwrap();
        assert_eq!(good.models.len(), 2);
        assert!(good.models[0].scoped);
        assert!(!good.models[1].scoped);
        shell.on(
            COMMAND,
            r#"{"type":"response","id":"models","success":false}"#,
        );
        assert!(catalogue.refresh(&shell, HOUR_MS - 1).is_none());
        let failed = catalogue.refresh(&shell, HOUR_MS).unwrap();
        assert_eq!(failed.models, good.models);
        assert!(failed.error.is_some());
        shell.on_output(
            COMMAND,
            Output {
                status: 124,
                stdout: TRANSCRIPT.into(),
                stderr: "timeout".into(),
            },
        );
        let timeout = catalogue.refresh(&shell, HOUR_MS * 2).unwrap();
        assert_eq!(timeout.models, good.models);
        assert!(timeout.error.as_deref().unwrap().contains("timeout"));
        shell.on(
            COMMAND,
            r#"{"type":"response","id":"models","success":true,"data":{"entries":[{"type":"custom","customType":"dogsled.model-catalogue.v1","data":[]}]}}"#,
        );
        assert!(catalogue.refresh(&shell, HOUR_MS * 2 + 1).is_none());
        assert_eq!(
            catalogue.refresh(&shell, HOUR_MS * 3).unwrap(),
            ModelCatalogue::default()
        );
    }
}
