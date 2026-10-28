use super::*;
use serde_json::json;

fn question(kind: &str) -> Result<Question, serde_json::Error> {
    serde_json::from_value(json!({
        "id": "ui", "type": kind, "question": "What renders the web UI?",
        "options": ["React", {"label": "Leptos"}], "recommended": "React"
    }))
}

#[test]
fn every_choice_question_offers_other_without_duplicates() -> Result<(), serde_json::Error> {
    for kind in ["single", "multi"] {
        let mut question = question(kind)?;
        assert_eq!(
            choice_options(&question)
                .iter()
                .map(option_label)
                .collect::<Vec<_>>(),
            ["React", "Leptos", "Other"]
        );
        question.options = Some(vec![json!({"label": "Other", "content": "Your proposal"})]);
        assert_eq!(
            choice_options(&question),
            question.options.clone().unwrap_or_default()
        );
        question.options = None;
        assert_eq!(choice_options(&question), [json!("Other")]);
    }
    Ok(())
}

#[test]
fn custom_answer_replaces_recommended_choice_and_survives_submission()
-> Result<(), serde_json::Error> {
    for kind in ["single", "multi"] {
        let questions = [question(kind)?];
        let multi = kind == "multi";
        let mut values = initial_values(&questions);
        if multi {
            choose(&mut values, "ui", "React", true);
        }
        choose(&mut values, "ui", "Other", multi);
        set_note(
            &mut values,
            "ui",
            "Other",
            "Dioxus\nKeep the UI in Rust.".to_owned(),
        );
        assert_eq!(chosen(&values, "ui"), ["Other"]);
        assert_eq!(answered_count(&questions, &values), 1);
        let expected = json!({"option": "Other", "note": "Dioxus\nKeep the UI in Rust."});
        assert_eq!(
            collect_responses(&questions, &values)[0].value,
            if multi { json!([expected]) } else { expected }
        );
    }
    Ok(())
}

#[test]
fn selected_choices_keep_context_when_another_choice_is_toggled() -> Result<(), serde_json::Error> {
    let questions = [question("multi")?];
    let mut values = initial_values(&questions);
    set_note(
        &mut values,
        "ui",
        "React",
        "Only for the existing shell".to_owned(),
    );
    choose(&mut values, "ui", "Other", true);
    set_note(&mut values, "ui", "Other", "Dioxus".to_owned());
    choose(&mut values, "ui", "Leptos", true);
    choose(&mut values, "ui", "Leptos", true);
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!([
            {"option": "React", "note": "Only for the existing shell"},
            {"option": "Other", "note": "Dioxus"}
        ])
    );
    choose(&mut values, "ui", "Other", true);
    assert_eq!(chosen(&values, "ui"), ["React"]);
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!([{"option": "React", "note": "Only for the existing shell"}])
    );
    Ok(())
}

#[test]
fn single_choice_context_can_be_cleared_or_replaced() -> Result<(), serde_json::Error> {
    let questions = [question("single")?];
    let mut values = initial_values(&questions);
    set_note(&mut values, "ui", "React", "Keep the current UI".to_owned());
    choose(&mut values, "ui", "React", false);
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!({"option": "React", "note": "Keep the current UI"})
    );
    set_note(&mut values, "ui", "React", String::new());
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!("React")
    );
    choose(&mut values, "ui", "Other", false);
    set_note(&mut values, "ui", "Other", "Dioxus".to_owned());
    choose(&mut values, "ui", "Leptos", false);
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!("Leptos")
    );
    Ok(())
}

#[test]
fn plain_answers_and_recommendations_keep_their_existing_shape() -> Result<(), serde_json::Error> {
    for (kind, expected) in [("single", json!("React")), ("multi", json!(["React"]))] {
        let mut question = question(kind)?;
        let questions = std::slice::from_ref(&question);
        assert_eq!(
            collect_responses(questions, &initial_values(questions))[0].value,
            expected
        );
        question.conviction = Some(json!("slight"));
        assert!(initial_values(&[question]).is_empty());
    }
    let questions = [question("text")?, question("info")?];
    let mut values = BTreeMap::from([("ui".to_owned(), json!("Dioxus"))]);
    assert_eq!(collect_responses(&questions, &values).len(), 1);
    assert_eq!(
        collect_responses(&questions, &values)[0].value,
        json!("Dioxus")
    );
    values.insert("ui".to_owned(), json!("  "));
    assert_eq!(answered_count(&questions, &values), 0);
    Ok(())
}
