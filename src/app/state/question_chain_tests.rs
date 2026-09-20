use super::{AppState, PendingQuestion, format_question_chain_answers};

fn chain() -> Vec<PendingQuestion> {
    vec![
        PendingQuestion::new("First?".to_owned(), vec!["a".to_owned()], false)
            .with_header("One".to_owned()),
        PendingQuestion::new(
            "Second?".to_owned(),
            vec!["b".to_owned(), "c".to_owned()],
            true,
        )
        .with_header("Two".to_owned()),
        PendingQuestion::new("Third?".to_owned(), vec!["d".to_owned()], false)
            .with_header("Three".to_owned()),
    ]
}

#[test]
fn chain_begin_positions_and_counts() {
    let mut state = AppState::new();
    state.begin_question_chain(chain());

    assert_eq!(state.question_chain_len(), 3);
    assert_eq!(state.question_chain_position(), 1);
    assert_eq!(state.question_chain_answered(), 0);
    assert_eq!(state.pending_question.as_ref().unwrap().header, "One");
}

#[test]
fn chain_advance_records_answers_in_order() {
    let mut state = AppState::new();
    state.begin_question_chain(chain());

    assert!(state.advance_question_chain("a".to_owned()));
    assert_eq!(state.question_chain_position(), 2);
    assert_eq!(state.question_chain_answered(), 1);
    assert!(state.advance_question_chain("b, c".to_owned()));
    assert!(!state.advance_question_chain("d".to_owned()));
    assert!(state.pending_question.is_none());
    assert!(state.pending_question_queue.is_empty());

    let answers = state.take_question_chain_answers(None);
    assert_eq!(
        answers,
        vec![
            ("One".to_owned(), "First?".to_owned(), "a".to_owned()),
            ("Two".to_owned(), "Second?".to_owned(), "b, c".to_owned()),
            ("Three".to_owned(), "Third?".to_owned(), "d".to_owned()),
        ]
    );
}

#[test]
fn chain_focus_moves_without_answering_and_reanswer_keeps_order() {
    let mut state = AppState::new();
    state.begin_question_chain(chain());

    // Answer Q1 (advance to Q2), then step back to Q1 without answering.
    assert!(state.advance_question_chain("a".to_owned()));
    state.focus_question(-1);
    assert_eq!(state.question_chain_position(), 1);
    assert_eq!(state.question_chain_answered(), 1);
    // Highlight/ticks/answer travel with the question.
    assert_eq!(
        state.pending_question.as_ref().unwrap().answer.as_deref(),
        Some("a")
    );

    // Re-answer Q1, advance through the rest: original order is preserved.
    assert!(state.advance_question_chain("a2".to_owned()));
    assert_eq!(state.question_chain_position(), 2);
    state.focus_question(1);
    assert_eq!(state.question_chain_position(), 3);
    let answers = state.take_question_chain_answers(Some("d".to_owned()));
    let headers = answers
        .iter()
        .map(|(header, _, _)| header.clone())
        .collect::<Vec<_>>();
    assert_eq!(headers, vec!["One", "Two", "Three"]);
    assert_eq!(answers[0].2, "a2");
    assert_eq!(answers[1].2, "");
    assert_eq!(answers[2].2, "d");
}

#[test]
fn single_question_chain_matches_legacy_positions() {
    let mut state = AppState::new();
    state.begin_question_chain(vec![PendingQuestion::new(
        "Only?".to_owned(),
        vec!["yes".to_owned()],
        false,
    )]);

    assert_eq!(state.question_chain_len(), 1);
    assert_eq!(state.question_chain_position(), 1);
    state.focus_question(1);
    assert_eq!(state.question_chain_position(), 1);
    assert!(state.pending_question.is_some());
}

#[test]
fn chain_answer_formatting_keeps_legacy_single_shape() {
    assert_eq!(
        format_question_chain_answers(&[(String::new(), String::new(), "X".to_owned())]),
        "User selected: X"
    );
    assert_eq!(
        format_question_chain_answers(&[
            ("H1".to_owned(), "Q1?".to_owned(), "A1".to_owned()),
            ("H2".to_owned(), "Q2?".to_owned(), String::new()),
        ]),
        "User answers:\n[H1] Q1? → A1\n[H2] Q2? → (skipped)"
    );
}

#[test]
fn question_options_carry_descriptions_and_display_answer() {
    let question = PendingQuestion::new(
        "Pick?".to_owned(),
        vec!["a".to_owned(), "b".to_owned()],
        true,
    )
    .with_descriptions(vec!["first".to_owned(), String::new()]);
    assert_eq!(question.description(0), Some("first"));
    assert_eq!(question.description(1), None);
    assert_eq!(question.description(9), None);

    let mut ticked = question.clone();
    ticked.chosen[0] = true;
    assert_eq!(ticked.display_answer(), "a");
}
