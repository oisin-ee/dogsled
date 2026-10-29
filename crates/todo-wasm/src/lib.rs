//! wasm-bindgen bindings of `todo-core`. Each export maps one tool operation;
//! a rejected operation throws a JavaScript `Error` with the core's message.
//!
//! Values cross the boundary as `tsify::Ts`, so a malformed value from
//! JavaScript fails as an ordinary `Result` inside the function and leaks
//! nothing (tsify README, "Why are the `wasm_abi` attributes deprecated").

use todo_core::{
    Change, MutationContext, ReminderHistory, Request, TodoList, TodoSection, WidgetRow,
};
use tsify::{Ts, Tsify};
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::{JsError, wasm_bindgen};

/// Replaces the whole list with `items`.
///
/// # Errors
///
/// Throws the core's message for an empty list, an empty task, or a repeat.
#[wasm_bindgen]
pub fn init(items: Vec<String>) -> Result<Ts<TodoList>, JsError> {
    let list = todo_core::init(items)?;
    Ok(list.into_ts()?)
}

/// Validates a stored snapshot. It comes from the session file, so it is typed
/// `unknown` and checked here, by the type that owns its shape.
///
/// # Errors
///
/// Throws when `snapshot` is not a valid todo list.
#[wasm_bindgen]
pub fn parse(
    #[wasm_bindgen(unchecked_param_type = "unknown")] snapshot: JsValue,
) -> Result<Ts<TodoList>, JsError> {
    let list: TodoList = Ts::<TodoList>::new_unchecked(snapshot).to_rust()?;
    todo_core::validate(&list)?;
    Ok(list.into_ts()?)
}

/// Applies one validated operation.
///
/// # Errors
///
/// Throws for malformed input or a rejected operation.
#[wasm_bindgen]
pub fn apply(
    list: &Ts<TodoList>,
    #[wasm_bindgen(unchecked_param_type = "unknown")] request: JsValue,
    context: &Ts<MutationContext>,
) -> Result<Ts<Change>, JsError> {
    let request = Ts::<Request>::new_unchecked(request).to_rust()?;
    Ok(todo_core::apply(list.to_rust()?, request, &context.to_rust()?)?.into_ts()?)
}

/// Reads a complete task contract.
///
/// # Errors
/// Rejects unknown IDs or incomplete contracts.
#[wasm_bindgen]
pub fn inspect(list: &Ts<TodoList>, id: &str) -> Result<Ts<todo_core::UnitContract>, JsError> {
    Ok(todo_core::inspect(&list.to_rust()?, id)?.into_ts()?)
}

/// Reads validated current-session activation.
///
/// # Errors
/// Rejects invalid persisted state.
#[wasm_bindgen]
pub fn active(
    list: &Ts<TodoList>,
    session_id: &str,
) -> Result<Option<Ts<todo_core::ActiveUnit>>, JsError> {
    todo_core::active(&list.to_rust()?, session_id)?
        .map(|value| value.into_ts().map_err(JsError::from))
        .transpose()
}

/// Validates and atomically freezes a worker contract.
///
/// # Errors
/// Rejects malformed requests, stale origins and competing claims.
#[wasm_bindgen]
pub fn claim(
    list: &Ts<TodoList>,
    input: &str,
    session_id: &str,
) -> Result<Ts<todo_core::ClaimChange>, JsError> {
    let input = todo_core::parse_claim(input)?;
    Ok(todo_core::claim(list.to_rust()?, &input, session_id)?.into_ts()?)
}

/// Imports one validated seed into a fresh worker session.
///
/// # Errors
/// Rejects invalid seed, repository identity or existing work.
#[wasm_bindgen]
pub fn import_unit(
    list: &Ts<TodoList>,
    input: &str,
    session_id: &str,
    repository_id: &str,
) -> Result<Ts<Change>, JsError> {
    let seed = todo_core::parse_seed(input)?;
    Ok(todo_core::import_unit(list.to_rust()?, seed, session_id, repository_id)?.into_ts()?)
}

/// Reads immutable import acknowledgement and closure evidence.
///
/// # Errors
/// Rejects invalid persisted state.
#[wasm_bindgen]
pub fn imported(
    list: &Ts<TodoList>,
    session_id: &str,
) -> Result<Option<Ts<todo_core::ImportReceipt>>, JsError> {
    todo_core::imported(&list.to_rust()?, session_id)?
        .map(|value| value.into_ts().map_err(JsError::from))
        .transpose()
}

/// Records an executor's exact release evidence, never acceptance.
///
/// # Errors
/// Rejects malformed or mismatched settlements.
#[wasm_bindgen]
pub fn settle(
    list: &Ts<TodoList>,
    input: &str,
    session_id: &str,
) -> Result<Ts<todo_core::SettlementChange>, JsError> {
    let input = todo_core::parse_settlement(input)?;
    Ok(todo_core::settle(list.to_rust()?, &input, session_id)?.into_ts()?)
}

/// Prevents mutations of revisions with an in-flight tool call.
///
/// # Errors
/// Rejects closure, replacement or alteration of any leased revision.
#[wasm_bindgen]
pub fn protect(
    before: &Ts<TodoList>,
    after: &Ts<TodoList>,
    leases: Vec<Ts<todo_core::UnitRef>>,
) -> Result<(), JsError> {
    let leases = leases
        .into_iter()
        .map(|lease| lease.to_rust())
        .collect::<Result<Vec<_>, _>>()?;
    Ok(todo_core::protect(
        &before.to_rust()?,
        &after.to_rust()?,
        &leases,
    )?)
}

/// Computes completion policy from branch facts.
///
/// # Errors
///
/// Throws for malformed facts.
#[wasm_bindgen]
pub fn completion_reminder(facts: &Ts<ReminderHistory>) -> Result<Option<String>, JsError> {
    Ok(todo_core::completion_reminder(&facts.to_rust()?.facts))
}

/// The model-facing summary of `list`.
///
/// # Errors
///
/// Throws when `list` is not a valid todo list.
#[wasm_bindgen]
pub fn view(list: &Ts<TodoList>) -> Result<String, JsError> {
    Ok(todo_core::view(&list.to_rust()?))
}

/// Typed status groups of `list`: always four sections, in a fixed order.
///
/// # Errors
///
/// Throws when `list` is not a valid todo list.
#[wasm_bindgen]
pub fn sections(list: &Ts<TodoList>) -> Result<Vec<Ts<TodoSection>>, JsError> {
    todo_core::sections(&list.to_rust()?)
        .into_iter()
        .map(|section| section.into_ts().map_err(JsError::from))
        .collect()
}

/// Human-facing rows for `list`, or `undefined` for an empty list.
///
/// # Errors
///
/// Throws when `list` is not a valid todo list.
#[wasm_bindgen]
pub fn widget(list: &Ts<TodoList>, expanded: bool) -> Result<Option<Vec<Ts<WidgetRow>>>, JsError> {
    todo_core::widget(&list.to_rust()?, expanded)
        .map(|rows| {
            rows.into_iter()
                .map(|row| row.into_ts().map_err(JsError::from))
                .collect()
        })
        .transpose()
}
