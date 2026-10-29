//! Config validation and top-level project overrides.

use crate::types::{Config, ConfigReport};
use serde_json::Value;

/// Reads two JSON documents. Project keys replace user keys, not nested fields.
/// Missing files are represented by `None`.
#[must_use]
pub fn configure(user: Option<&str>, project: Option<&str>) -> ConfigReport {
    let mut errors = Vec::new();
    let mut merged = serde_json::Map::new();
    for (name, source) in [
        ("User prewalk.json", user),
        ("Project prewalk.json", project),
    ] {
        let Some(source) = source else { continue };
        match parse(source) {
            Ok(Value::Object(values)) => merged.extend(values),
            Ok(_) => errors.push(format!("{name}: expected an object")),
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    let config = if errors.is_empty() {
        match serde_json::from_value(Value::Object(merged)) {
            Ok(config) => config,
            Err(error) => {
                errors.push(format!("prewalk.json: {error}"));
                Config::default()
            }
        }
    } else {
        Config::default()
    };
    ConfigReport { config, errors }
}

fn parse(source: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(source).map_err(|error| error.to_string())?;
    let config: Config =
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
    if let Some(target) = config.target {
        validate_model(&target.model)?;
    }
    Ok(value)
}

/// Validates a provider-qualified model without constraining provider model IDs.
///
/// # Errors
/// Returns a message for an empty provider or model, or whitespace.
pub fn validate_model(model: &str) -> Result<(), String> {
    if model.trim() != model || model.contains(char::is_whitespace) {
        return Err("target.model must not contain whitespace".into());
    }
    match model.split_once('/') {
        Some((provider, id)) if !provider.is_empty() && !id.is_empty() => Ok(()),
        _ => Err("target.model must be provider/model".into()),
    }
}
