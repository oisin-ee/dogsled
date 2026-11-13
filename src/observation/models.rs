//! Serve-owned Pi model catalogue. Readers consume the retained publication.

use crate::error::{Error, Result};
use crate::shell::Shell;
use portal_types::models::{ModelCatalogue, ModelChoice, ModelRef, ThinkingLevel};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const HOUR_MS: u64 = 60 * 60 * 1000;
const INPUT: &[u8] = b"{\"id\":\"models\",\"type\":\"get_available_models\"}\n";

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

#[derive(Deserialize)]
struct ChatModel {
    provider: String,
    id: String,
    name: Option<String>,
    #[serde(default)]
    reasoning: bool,
    #[serde(default, rename = "thinkingLevelMap")]
    thinking_level_map: BTreeMap<String, Value>,
}

impl ChatModel {
    fn choice(self) -> ModelChoice {
        use ThinkingLevel::{High, Low, Max, Medium, Minimal, Off, Xhigh};
        // Port of getSupportedThinkingLevels in @earendil-works/pi-ai 1.3.0,
        // dist/models.js: null disables a level; xhigh/max need an explicit mapping.
        let thinking = if self.reasoning {
            [Off, Minimal, Low, Medium, High, Xhigh, Max]
                .into_iter()
                .filter(|level| {
                    let mapped = self.thinking_level_map.get(level.as_str());
                    !matches!(mapped, Some(Value::Null))
                        && (!matches!(level, Xhigh | Max) || mapped.is_some())
                })
                .collect()
        } else {
            vec![Off]
        };
        ModelChoice {
            model: ModelRef {
                provider: self.provider,
                id: self.id,
            },
            name: self.name,
            thinking,
        }
    }
}

fn read(shell: &dyn Shell) -> Result<Vec<ModelChoice>> {
    let output = shell.run_input_bounded(
        "pi",
        &["--mode".into(), "rpc".into(), "--no-session".into()],
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
        let models = response["data"]["models"]
            .as_array()
            .ok_or_else(|| Error::Invalid("Pi model catalogue: missing models array".into()))?;
        return models
            .iter()
            .filter(|model| model["type"] == "chat")
            .map(|model| {
                serde_json::from_value::<ChatModel>(model.clone())
                    .map(ChatModel::choice)
                    .map_err(Into::into)
            })
            .collect();
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
{"type":"response","id":"models","command":"get_available_models","success":true,"data":{"models":[{"type":"chat","provider":"openai","id":"gpt-5","name":"GPT-5","reasoning":true,"thinkingLevelMap":{"off":null,"minimal":null,"xhigh":"xhigh","max":null}},{"type":"chat","provider":"anthropic","id":"claude","name":"Claude","reasoning":true,"thinkingLevelMap":{"max":"max"}},{"type":"chat","provider":"other","id":"plain","name":"Plain","reasoning":false,"thinkingLevelMap":{"xhigh":"high"}},{"type":"image","id":"image"}]}}
{"type":"extension_ui_request","id":"ui-2","method":"setWidget"}"#;
    const COMMAND: &str = "pi --mode rpc --no-session";

    #[test]
    fn rpc_transcript_filters_models_and_ports_thinking_levels() {
        use ThinkingLevel::{High, Low, Max, Medium, Minimal, Off, Xhigh};
        let models = parse(TRANSCRIPT).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].model.pattern(), "openai/gpt-5");
        assert_eq!(models[0].name.as_deref(), Some("GPT-5"));
        assert_eq!(models[0].thinking, [Low, Medium, High, Xhigh]);
        assert_eq!(models[1].thinking, [Off, Minimal, Low, Medium, High, Max]);
        assert_eq!(models[2].thinking, [Off]);
        let default = parse(r#"{"id":"models","type":"response","success":true,"data":{"models":[{"type":"chat","provider":"p","id":"m","reasoning":true}]}}"#).unwrap();
        assert_eq!(default[0].thinking, [Off, Minimal, Low, Medium, High]);
    }

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
            assert_eq!(args, ["--mode", "rpc", "--no-session"]);
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
        assert_eq!(good.models.len(), 3);
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
            r#"{"type":"response","id":"models","success":true,"data":{"models":[]}}"#,
        );
        assert!(catalogue.refresh(&shell, HOUR_MS * 2 + 1).is_none());
        assert_eq!(
            catalogue.refresh(&shell, HOUR_MS * 3).unwrap(),
            ModelCatalogue::default()
        );
    }
}
