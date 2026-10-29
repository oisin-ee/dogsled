//! Checked JS inputs and generated TypeScript outputs for the pure protocol.

use prewalk_core::types::{Branch, ConfigReport, Decision, Facts, Projection, State};
use tsify::{Ts, Tsify};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::{JsError, wasm_bindgen};

/// Validates facts and returns ordered adapter actions.
///
/// # Errors
/// Throws when the input does not match the protocol schema.
#[wasm_bindgen]
pub fn decide(
    #[wasm_bindgen(unchecked_param_type = "unknown")] facts: JsValue,
) -> Result<Ts<Decision>, JsError> {
    let facts = Ts::<Facts>::new_unchecked(facts).to_rust()?;
    Ok(prewalk_core::decide(&facts).into_ts()?)
}

/// Decides context projection from branch evidence.
///
/// # Errors
/// Throws when the input is not a valid branch.
#[wasm_bindgen]
pub fn project(
    #[wasm_bindgen(unchecked_param_type = "unknown")] branch: JsValue,
) -> Result<Ts<Projection>, JsError> {
    let branch = Ts::<Branch>::new_unchecked(branch).to_rust()?;
    Ok(prewalk_core::project(&branch.records).into_ts()?)
}

/// Validates a state loaded from a session entry.
///
/// # Errors
/// Throws when the stored value is not a protocol state.
#[wasm_bindgen]
pub fn parse_state(
    #[wasm_bindgen(unchecked_param_type = "unknown")] state: JsValue,
) -> Result<Ts<State>, JsError> {
    Ok(Ts::<State>::new_unchecked(state).to_rust()?.into_ts()?)
}

/// Validates user and trusted project JSON documents.
///
/// # Errors
/// Throws only when serialization to JS fails. Config errors are returned.
#[expect(
    clippy::needless_pass_by_value,
    reason = "wasm-bindgen accepts owned optional strings, not Option<&str>"
)]
#[wasm_bindgen]
pub fn configure(
    user: Option<String>,
    project: Option<String>,
) -> Result<Ts<ConfigReport>, JsError> {
    Ok(prewalk_core::configure(user.as_deref(), project.as_deref()).into_ts()?)
}
