use super::*;
use crate::ui::render_snapshot::{render_snapshot, set_current_response};

fn spawn_background_task_for_test(
    task_id: &str,
    session_id: &str,
    command: &str,
) -> Result<(), String> {
    // Route through the controller contract so the render layer never names
    // engine task internals directly (frontend seam #1431/#1442).
    rustcode::controller::spawn_background_task(task_id, session_id, command)
}

pub(crate) static THEME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Width handed to `activity_status_line` by tests that assert hint content
/// rather than how the line degrades: wide enough that every clause survives
/// (#1529).
const ROOMY_ACTIVITY_WIDTH: usize = 200;

fn render_state_to_text(state: &mut RenderState, width: u16, height: u16) -> String {
    let (text, _) = render_state_to_text_with_composer_area(state, width, height);
    text
}

/// Render one frame and report where the composer landed.
///
/// The view carries no engine-side layout metrics, so tests that assert the
/// composer's screen position read the rect the renderer returned.
fn render_state_to_text_with_composer_area(
    state: &mut RenderState,
    width: u16,
    height: u16,
) -> (String, ratatui::layout::Rect) {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut input_area = ratatui::layout::Rect::default();
    terminal
        .draw(|frame| input_area = render(frame, state).1)
        .unwrap();

    let text = (0..height)
        .map(|row| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, input_area)
}

fn render_state_to_text_with_transcript(
    state: &mut RenderState,
    transcript: &mut TranscriptState,
    width: u16,
    height: u16,
) -> String {
    let (text, _) =
        render_state_to_text_with_transcript_and_composer_area(state, transcript, width, height);
    text
}

fn render_state_to_text_with_transcript_and_composer_area(
    state: &mut RenderState,
    transcript: &mut TranscriptState,
    width: u16,
    height: u16,
) -> (String, ratatui::layout::Rect) {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut input_area = ratatui::layout::Rect::default();
    terminal
        .draw(|frame| input_area = render_with_transcript(frame, state, transcript).1)
        .unwrap();
    let text = (0..height)
        .map(|row| {
            (0..width)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, input_area)
}

/// Row the composer footer is laid out on: the final terminal row.
fn footer_row(height: u16) -> u16 {
    height - 1
}

/// Drag a transcript selection the way the runtime does on a mouse gesture:
/// Down pins the painted viewport, Drag and Up leave a released selection
/// behind for the explicit copy chord to read (#1492).
fn select_transcript_text(
    state: &mut RenderState,
    transcript: &mut TranscriptState,
    from: (u16, u16),
    to: (u16, u16),
) {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let mouse = |kind: MouseEventKind, (column, row): (u16, u16)| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    transcript.selection.begin_with_snapshot(
        mouse(MouseEventKind::Down(MouseButton::Left), from),
        render_snapshot(state),
        transcript.scroll_rows(),
    );
    transcript
        .selection
        .mouse(mouse(MouseEventKind::Drag(MouseButton::Left), to));
    transcript
        .selection
        .mouse(mouse(MouseEventKind::Up(MouseButton::Left), to));
}

/// The footer row of a frame drawn with a live transcript selection.
///
/// The first frame paints the rows the selection pins, so the gesture has to
/// happen between two draws, exactly as it does between two events.
fn footer_row_with_transcript_selection(
    state: &mut RenderState,
    width: u16,
    height: u16,
) -> String {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            render_with_transcript(frame, state, &mut transcript);
        })
        .unwrap();
    select_transcript_text(state, &mut transcript, (2, 2), (20, 2));
    assert!(
        transcript.selection.has_selection(),
        "the drag must leave a non-empty selection"
    );
    terminal
        .draw(|frame| {
            render_with_transcript(frame, state, &mut transcript);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..width)
        .map(|column| buffer[(column, footer_row(height))].symbol())
        .collect()
}

fn render_context_modal_to_text(state: &RenderState, width: u16, height: u16) -> String {
    render_context_modal_to_buffer(state, width, height)
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One rendered cell: symbol plus the colours and modifiers the panel painted
/// it with. `/context` is a colour change, so assertions and screenshots both
/// need the appearance, not just the glyphs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderedCell {
    symbol: String,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    modifier: ratatui::style::Modifier,
}

/// Render `/context` into a `TestBackend` and keep each cell's appearance.
/// This is what makes a real image of the panel possible without a PTY: the
/// buffer carries truecolor `Color::Rgb` values, which `buffer_to_ansi` turns
/// into escape sequences and a screenshot turns into pixels.
fn render_context_modal_to_buffer(
    state: &RenderState,
    width: u16,
    height: u16,
) -> Vec<Vec<RenderedCell>> {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, layout::Rect};

    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_context_modal(
                frame,
                &snapshot,
                Rect::new(0, height.saturating_sub(3), width, 3),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|row| {
            (0..width)
                .map(|column| {
                    let cell = &buffer[(column, row)];
                    RenderedCell {
                        symbol: cell.symbol().to_owned(),
                        fg: cell.fg,
                        bg: cell.bg,
                        modifier: cell.modifier,
                    }
                })
                .collect()
        })
        .collect()
}

/// SGR truecolor for a `Color`, falling back to `fallback` for `Reset`.
fn sgr(color: ratatui::style::Color, fallback: ratatui::style::Color) -> String {
    let ratatui::style::Color::Rgb(r, g, b) = color else {
        let ratatui::style::Color::Rgb(r, g, b) = fallback else {
            return "39".to_owned();
        };
        return format!("38;2;{r};{g};{b}");
    };
    format!("38;2;{r};{g};{b}")
}

/// CSS colour for a cell. `Reset` means "whatever the terminal paints", which
/// inside the panel is the panel surface, so it falls back to that.
fn hex(color: ratatui::style::Color, fallback: ratatui::style::Color) -> String {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => hex(fallback, ratatui::style::Color::Rgb(0, 0, 0)),
    }
}

/// Drop rows the panel never painted, so the screenshot is the panel rather
/// than the empty transcript above it.
fn crop_to_panel(rows: Vec<Vec<RenderedCell>>) -> Vec<Vec<RenderedCell>> {
    let painted = |row: &Vec<RenderedCell>| row.iter().any(|c| c.symbol != " ");
    let first = rows.iter().position(painted).unwrap_or(0);
    let last = rows.iter().rposition(painted).unwrap_or(rows.len() - 1);
    rows[first..=last].to_vec()
}

/// The rendered buffer as a truecolor ANSI block. Only used for eyeballing a
/// terminal session; the screenshot path goes through `buffer_to_html`.
fn buffer_to_ansi(rows: &[Vec<RenderedCell>], panel: ratatui::style::Color) -> String {
    let mut out = String::new();
    for row in rows {
        out.push_str(&format!(
            "\x1b[48;2;{}m",
            match panel {
                ratatui::style::Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
                _ => "0;0;0".to_owned(),
            }
        ));
        for cell in row {
            out.push_str(&format!(
                "\x1b[{};{}m{}",
                sgr(cell.fg, ratatui::style::Color::Rgb(255, 255, 255)),
                match cell.bg {
                    ratatui::style::Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
                    _ => "49".to_owned(),
                },
                cell.symbol
            ));
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// The rendered buffer as a self-contained HTML page, one `<span>` per run of
/// identically-styled cells, so headless Chrome can turn it into a PNG with the
/// panel's real truecolor values.
fn buffer_to_html(rows: &[Vec<RenderedCell>], title: &str, panel: ratatui::style::Color) -> String {
    let rows = crop_to_panel(rows.to_vec());
    let rows = rows.as_slice();
    let mut body = String::new();
    for row in rows {
        body.push_str("<div class=\"row\">");
        let mut index = 0;
        while index < row.len() {
            let cell = &row[index];
            let mut end = index;
            while end < row.len()
                && row[end].fg == cell.fg
                && row[end].bg == cell.bg
                && row[end].modifier == cell.modifier
            {
                end += 1;
            }
            let text: String = row[index..end].iter().map(|c| c.symbol.as_str()).collect();
            let bold = if cell.modifier.contains(ratatui::style::Modifier::BOLD) {
                "font-weight:700;"
            } else {
                ""
            };
            body.push_str(&format!(
                "<span style=\"color:{};background:{};{bold}\">{}</span>",
                hex(cell.fg, panel),
                hex(cell.bg, panel),
                text.replace(' ', "&nbsp;")
            ));
            index = end;
        }
        body.push_str("</div>");
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
<style>body{{margin:0;padding:24px;background:#000}}\
.pre{{background:{panel};padding:20px 22px;border-radius:10px;display:inline-block}}\
.row{{font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;font-size:15px;\
line-height:19px;white-space:pre;color-scheme:light}}</style></head>\
<body><div class=\"pre\">{body}</div></body></html>",
        title = title,
        panel = hex(panel, ratatui::style::Color::Rgb(0, 0, 0)),
        body = body
    )
}

fn render_snapshot_to_text(state: &RenderState, width: u16, height: u16) -> String {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct CellAppearance {
        foreground: ratatui::style::Color,
        background: ratatui::style::Color,
        modifiers: ratatui::style::Modifier,
    }

    impl From<&ratatui::buffer::Cell> for CellAppearance {
        fn from(cell: &ratatui::buffer::Cell) -> Self {
            Self {
                foreground: cell.fg,
                background: cell.bg,
                modifiers: cell.modifier,
            }
        }
    }

    let snapshot = render_snapshot(&state);
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut appearances = Vec::new();
    let mut symbol_rows = Vec::with_capacity(height as usize);
    let mut appearance_rows = Vec::with_capacity(height as usize);

    for row in 0..height {
        let mut symbols = String::new();
        let mut row_appearances = Vec::with_capacity(width as usize);

        for column in 0..width {
            let cell = &buffer[(column, row)];
            symbols.push_str(cell.symbol());

            let appearance = CellAppearance::from(cell);
            let appearance_id = appearances
                .iter()
                .position(|existing| *existing == appearance)
                .unwrap_or_else(|| {
                    appearances.push(appearance);
                    appearances.len() - 1
                });
            row_appearances.push(format!("{appearance_id:02}"));
        }

        symbol_rows.push(symbols);
        appearance_rows.push(row_appearances.join(" "));
    }

    let appearance_palette = appearances
        .iter()
        .enumerate()
        .map(|(id, appearance)| {
            format!(
                "{id:02}: fg={:?}, bg={:?}, modifiers={:?}",
                appearance.foreground, appearance.background, appearance.modifiers
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "{}\n\ncell appearances:\n{}\n\nappearance palette:\n{}",
        symbol_rows.join("\n"),
        appearance_rows.join("\n"),
        appearance_palette
    )
}

#[test]
fn render_snapshot_preserves_existing_ui_output() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut states = Vec::new();

    states.push(RenderState::new());

    let mut streaming = RenderState::new();
    streaming.history.push(ChatMessage::new("user", "hello"));
    streaming.status = AppStatus::Streaming;
    set_current_response(&mut streaming, "streamed output");
    states.push(streaming);

    let mut approval = RenderState::new();
    approval.status = AppStatus::AwaitingToolConfirmation;
    approval.pending_tool_confirmation = Some(vec![rustcode::controller::ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: None,
    }]);
    states.push(approval);

    let mut question = RenderState::new();
    question.status = AppStatus::AwaitingQuestion;
    question.pending_question = Some(rustcode::controller::PendingQuestion::new(
        "Proceed?".to_owned(),
        vec!["Yes".to_owned(), "No".to_owned()],
        false,
    ));
    states.push(question);

    let mut picker = RenderState::new();
    picker.show_model_picker = true;
    states.push(picker);

    let mut selected_subagent = RenderState::new();
    let child = rustcode::controller::SubAgentView {
        id: 7,
        name: "reviewer".to_owned(),
        task: "review the patch".to_owned(),
        history: std::sync::Arc::new(vec![ChatMessage::new("assistant", "subagent response")]),
        status: rustcode::controller::SubAgentStatus::Running,
        active_turn: true,
        parent_id: Some(3),
    };
    selected_subagent.subagents.push(child.clone());
    selected_subagent.selected_subagent = Some(child);
    selected_subagent.selected_subagent_id = Some(7);
    states.push(selected_subagent);

    states[0].active_session_id = "session-test-123".to_owned();

    for state in &mut states {
        state.config = rustcode::controller::AppConfig::default();
        state.model_name = "gemini-3.6-flash".to_owned();
        state.api_base_url = "http://localhost:3000/v1/chat/completions".to_owned();
        state.cwd_and_branch = "/repo:main".to_owned();
    }
    theme::set_active_theme("default");

    let appearance_oracle = render_snapshot_to_text(&states[0], 1, 1);
    assert!(
        appearance_oracle.contains("fg=")
            && appearance_oracle.contains("bg=")
            && appearance_oracle.contains("modifiers="),
        "render oracle must include complete cell appearance"
    );

    // These fixtures are the independent rendering oracle: changing the
    // snapshot renderer changes a terminal cell and fails this test.
    fn expand_golden_fixture(fixture: &str) -> String {
        let version = env!("CARGO_PKG_VERSION");
        fixture
            .trim_end()
            .replace('␠', " ")
            .lines()
            .map(|line| {
                let Some(version_start) = line.find("v0.00.0") else {
                    return line.to_owned();
                };
                let version_end = version_start + "v0.00.0".len();
                let suffix = &line[version_end..];
                let Some(border_start) = suffix.rfind('╮') else {
                    return line.to_owned();
                };

                let fill_count = suffix[..border_start]
                    .chars()
                    .filter(|&character| character == '─')
                    .count();
                let version_delta = version.len() as isize - "0.00.0".len() as isize;
                let adjusted_fill = (fill_count as isize - version_delta).max(0) as usize;
                let trailing = &suffix[border_start + '╮'.len_utf8()..];

                format!(
                    "{}v{} {}╮{}",
                    &line[..version_start],
                    version,
                    "─".repeat(adjusted_fill),
                    trailing
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    let golden_outputs = [
        expand_golden_fixture(include_str!("fixtures/render_snapshot_0.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_1.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_2.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_3.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_4.txt")),
        expand_golden_fixture(include_str!("fixtures/render_snapshot_5.txt")),
    ];

    for (index, state) in states.into_iter().enumerate() {
        let actual = render_snapshot_to_text(&state, 60, 16);

        assert_eq!(
            actual, golden_outputs[index],
            "render case {index} diverged"
        );
    }
}

#[test]
fn acceptance_empty_session_has_welcome_and_composer() {
    let mut state = RenderState::new();
    let rendered = render_state_to_text(&mut state, 100, 28);

    assert!(
        rendered.contains(">_ RustCode") && rendered.contains("model:"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("Ask RustCode to do anything"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn acceptance_streaming_session_has_working_surface_and_live_text() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "streamed output");

    let rendered = render_state_to_text(&mut state, 100, 20);

    assert!(!rendered.contains("Working"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("streamed output"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn working_indicator_lives_in_the_chat_not_below_the_composer() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    // Prior turns keep the welcome banner out of the projection, so the chat
    // has a genuinely spare trailing row for the indicator.
    state.history.push(ChatMessage::new("user", "hello"));
    state
        .history
        .push(ChatMessage::new("assistant", "earlier answer"));
    set_current_response(&mut state, "streamed output");
    let mut transcript = TranscriptState::default();
    let (rendered, input_area) =
        render_state_to_text_with_transcript_and_composer_area(&mut state, &mut transcript, 60, 30);
    let rows = rendered.lines().collect::<Vec<_>>();

    // No status row below the composer any more.
    let below_composer = input_area.bottom() as usize;
    assert!(
        !rows[below_composer]
            .chars()
            .any(|c| "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(c)),
        "spinner must not render below the composer: {rendered:?}"
    );
    assert_eq!(rendered.matches("Working").count(), 0);

    // The chat carries the running indicator instead.
    let chat_rows = &rows[..input_area.y as usize];
    assert!(
        chat_rows.iter().any(|row| row.contains(&state.model_name)),
        "the chat must carry the running indicator: {rendered:?}"
    );
}

#[test]
fn a_full_chat_keeps_the_running_indicator() {
    // Reserve activity rows even when streaming text fills the viewport.
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.history.push(ChatMessage::new("user", "hello"));
    set_current_response(
        &mut state,
        (1..=20)
            .map(|index| format!("response line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut transcript = TranscriptState::default();
    let (rendered, input_area) =
        render_state_to_text_with_transcript_and_composer_area(&mut state, &mut transcript, 50, 12);
    let chat_rows = rendered
        .lines()
        .take(input_area.y as usize)
        .collect::<Vec<_>>();
    assert!(
        chat_rows.iter().any(|row| row.contains(&state.model_name)),
        "a full chat must keep its running indicator: {rendered:?}"
    );
}

#[test]
fn acceptance_tool_confirmation_replaces_composer_with_actions() {
    use rustcode::controller::ToolConfirmation;

    let mut state = RenderState::new();
    state.status = AppStatus::AwaitingToolConfirmation;
    state.pending_tool_confirmation = Some(vec![ToolConfirmation {
        request_id: None,
        tool_name: "run_command".to_owned(),
        path: "cargo test".to_owned(),
        content_preview: String::new(),
        content_bytes: 0,
        rememberable_prefix: Some("cargo test".to_owned()),
        forbidden_prefix: None,
    }]);

    let rendered = render_state_to_text(&mut state, 100, 14);

    assert!(
        rendered.contains("Would you like to run the following command?"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("$ cargo test"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Press enter to confirm"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn acceptance_narrow_terminal_keeps_the_composer_visible() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    let rendered = render_state_to_text(&mut state, 48, 8);

    assert!(rendered.contains("Ask RustCode"), "rendered: {rendered:?}");
}

#[test]
fn desired_height_keeps_the_composer_at_the_terminal_bottom() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    let mut transcript = TranscriptState::default();

    assert_eq!(super::desired_height(&state, &mut transcript, 100, 40), 40);
}

#[test]
fn short_live_reply_follows_the_welcome_cell() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "short reply");
    let rendered = render_state_to_text(&mut state, 80, 24);
    let lines = rendered.lines().collect::<Vec<_>>();
    let reply_row = lines
        .iter()
        .position(|line| line.contains("short reply"))
        .unwrap();
    let composer_row = lines
        .iter()
        .position(|line| line.contains("Ask RustCode to do anything"))
        .unwrap();
    let welcome_bottom = lines
        .iter()
        .position(|line| line.contains('╰'))
        .expect("welcome cell bottom border");
    assert!(reply_row > welcome_bottom, "rendered: {rendered:?}");
    assert!(composer_row > 18, "rendered: {rendered:?}");
}

#[test]
fn streaming_reply_keeps_earlier_lines_visible_in_the_full_viewport() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "First answer line\nSecond answer line");

    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(
        rendered.contains("First answer line"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("Second answer line"),
        "rendered: {rendered:?}"
    );
}

#[test]
fn static_slash_output_stays_visible_after_a_picker_closes() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("system", "RustCode build information"));
    state.show_model_picker = true;
    let picker = render_state_to_text(&mut state, 80, 24);
    assert!(picker.contains("Select model"));
    state.show_model_picker = false;
    let chat = render_state_to_text(&mut state, 80, 24);
    assert!(
        chat.contains("RustCode build information"),
        "rendered: {chat:?}"
    );
}

#[test]
fn transcript_scroll_moves_chat_without_changing_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    for index in 0..30 {
        state
            .history
            .push(ChatMessage::new("system", format!("entry {index}")));
    }
    state.input_buffer = "unchanged draft".to_owned();
    state.cursor_position = state.input_buffer.len();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let render_text = |terminal: &Terminal<TestBackend>| {
        (0..24)
            .map(|row| {
                (0..80)
                    .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let latest = render_text(&terminal);
    transcript.scroll_up(1);
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let older = render_text(&terminal);
    assert_ne!(latest, older);
    assert!(latest.contains("unchanged draft"));
    assert!(older.contains("unchanged draft"));
    assert_eq!(state.input_buffer, "unchanged draft");
}

#[test]
fn transient_notice_appears_below_input_without_entering_chat_history() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.transient_notice = Some("YOLO mode enabled".to_owned());
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("YOLO mode enabled"));
    assert!(state.history.is_empty());
}

#[test]
fn visible_transcript_groups_tools_from_one_batch_under_one_heading() {
    use rustcode::controller::{ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"echo ready"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "list_emails".to_owned(),
                arguments: "{}".to_owned(),
            },
        ]));
    for (id, name) in [("call-1", "run_command"), ("call-2", "list_emails")] {
        state.history.push(
            ChatMessage::new("tool", format!("{name}: ok"))
                .answering(Some(id.to_owned()))
                .with_tool_result(ToolResultRecord {
                    tool_name: name.to_owned(),
                    success: true,
                    ..Default::default()
                }),
        );
    }
    let snapshot = super::render_snapshot::render_snapshot(&state);
    let mut transcript = TranscriptState::default();
    let rendered =
        super::render_visible_conversation_with_transcript(&snapshot, 80, 20, &mut transcript)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("• Ran"))
            .count(),
        1
    );
    assert!(rendered.iter().any(|line| line.contains("Bash echo ready")));
    assert!(rendered.iter().any(|line| line.contains("ListEmails")));
}

#[test]
fn active_tool_does_not_repeat_a_committed_thought() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    let thought = "<think>Plan the shell command.</think>";
    state.history.push(ChatMessage::new("assistant", thought));
    set_current_response(&mut state, thought);
    state.status = AppStatus::Streaming;
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new("call-1", None, "run_command", "Bash", "sleep 10"),
    );
    let rendered = render_state_to_text(&mut state, 100, 24);
    assert_eq!(
        rendered.matches("Plan the shell command.").count(),
        1,
        "rendered: {rendered:?}"
    );
}

#[test]
fn desired_height_grows_with_streaming_text_and_clamps_to_terminal() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    state.status = AppStatus::Streaming;
    set_current_response(
        &mut state,
        (0..50)
            .map(|line| format!("streamed line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut transcript = TranscriptState::default();

    let height = super::desired_height(&state, &mut transcript, 40, 18);
    assert!(height > 6);
    assert_eq!(height, 18);
}

#[test]
fn model_picker_keeps_multiple_models_visible_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.config.models = (1..=5)
        .map(|number| rustcode::controller::ModelProfile {
            name: format!("model-{number}"),
            url: format!("http://localhost/{number}"),
            model: format!("model-{number}"),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        })
        .collect();
    state.show_model_picker = true;

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let visible_models = (1..=5)
        .filter(|number| rendered.contains(&format!("model-{number}")))
        .count();

    assert!(
        visible_models >= 3,
        "the inline picker must show several choices, got {visible_models}: {rendered:?}"
    );
}

#[test]
fn command_picker_keeps_multiple_commands_visible_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.show_command_picker = true;

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    let visible_commands = ["New session", "Fork session", "Archive session"]
        .iter()
        .filter(|command| rendered.contains(**command))
        .count();

    assert_eq!(
        visible_commands, 3,
        "the inline picker must show its first three commands: {rendered:?}"
    );
}

#[test]
fn inline_command_suggestions_render_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(0);

    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let row_text = |row: u16| {
        (0..100)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>()
    };
    let composer_row = (0..20)
        .rev()
        .find(|row| row_text(*row).contains("› /"))
        .expect("composer input row should be visible");
    let popup_row = (0..20)
        .find(|row| row_text(*row).contains("/cancel"))
        .expect("inline command popup should be visible");

    assert!(
        popup_row < composer_row,
        "popup should be above the composer: composer={composer_row}, popup={popup_row}"
    );
    let hint_row = (0..20)
        .find(|row| row_text(*row).contains("navigate"))
        .expect("completion navigation hint should be visible under the composer");
    assert!(
        hint_row > composer_row,
        "the hint belongs under the composer: composer={composer_row}, hint={hint_row}"
    );
    assert!(
        row_text(hint_row).contains("enter select"),
        "the hint should name the keys that select a command: {:?}",
        row_text(hint_row)
    );
}

#[test]
fn completion_footer_hint_replaces_session_metadata() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(0);
    let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
    terminal
        .draw(|frame| {
            super::render(frame, &mut state);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let row_text = |row: u16| {
        (0..120)
            .map(|column| buffer[(column, row)].symbol())
            .collect::<String>()
    };
    let hint_row = (0..20)
        .find(|row| row_text(*row).contains("navigate"))
        .expect("completion hint should be rendered");
    let location = super::composer_render::footer_location(&render_snapshot(&state));
    assert!(
        !row_text(hint_row).contains(location.trim()),
        "the hint replaces the session location on the left: {:?}",
        row_text(hint_row)
    );
    assert!(
        (0..20).any(|row| row_text(row).contains("esc dismiss")),
        "the hint should say how to dismiss the popup"
    );
}

/// The completion hint degrades by content as the row narrows: the keys that
/// act survive, the trailing clauses go first, and nothing is left half-printed
/// with an ellipsis (#1529).
#[test]
fn completion_hint_degrades_by_content_at_narrow_widths() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let hint_row = |terminal: &Terminal<TestBackend>, width: u16| {
        let buffer = terminal.backend().buffer();
        (0..terminal.backend().buffer().area.height)
            .map(|row| {
                (0..width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .find(|text| text.contains("navigate"))
            .unwrap_or_else(|| panic!("completion hint should be rendered at {width} columns"))
    };

    // The right-hand percentage is 19 columns wide in a fresh session, so the
    // hint has `width - 19` columns to work with before that text has to go.
    for (width, expected) in [
        (
            100u16,
            "  ↑/↓ navigate · enter select · tab complete · esc dismiss",
        ),
        (
            80,
            "  ↑/↓ navigate · enter select · tab complete · esc dismiss",
        ),
        (60, "  ↑/↓ navigate · enter select"),
        // Too narrow for the hint beside the percentage: the percentage yields.
        (30, "  ↑/↓ navigate · enter select"),
    ] {
        let mut state = RenderState::new();
        state.input_buffer = "/".to_owned();
        state.cursor_position = 1;
        state.active_suggestion_index = Some(0);

        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        terminal
            .draw(|frame| {
                super::render(frame, &mut state);
            })
            .unwrap();

        let row = hint_row(&terminal, width);
        assert!(
            row.starts_with(expected),
            "expected the hint to read {expected:?} at {width} columns, got {row:?}"
        );
        assert!(
            !row.contains('…'),
            "a hint must be omitted whole, never clipped with an ellipsis: {row:?}"
        );
        assert_eq!(
            row.chars().count(),
            width as usize,
            "the footer row must fill the viewport exactly once"
        );
    }
}

/// The context percentage is informational, so it is the first thing to go when
/// the hint needs the whole row (#1529).
#[test]
fn composer_footer_drops_the_context_percentage_before_a_hint_clause() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let footer_rows = |width: u16| {
        let mut state = RenderState::new();
        state.input_buffer = "/".to_owned();
        state.cursor_position = 1;
        state.active_suggestion_index = Some(0);
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        terminal
            .draw(|frame| {
                super::render(frame, &mut state);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..20)
            .map(|row| {
                (0..width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };

    // 30 columns cannot fit "↑/↓ navigate" (14) beside "100% context left  "
    // (19), so the percentage is dropped rather than a hint clause.
    let narrow = footer_rows(30);
    let hint = narrow
        .iter()
        .find(|row| row.contains("navigate"))
        .expect("hint row at 30 columns");
    assert!(hint.contains("enter select"));
    assert!(
        !narrow.iter().any(|row| row.contains("context left")),
        "the context percentage must yield to the hint: {narrow:?}"
    );

    // At 60 columns both fit, so neither is sacrificed.
    let roomy = footer_rows(60);
    assert!(roomy.iter().any(|row| row.contains("context left")));
    assert!(roomy.iter().any(|row| row.contains("enter select")));
}

#[test]
fn command_popup_keeps_composer_on_the_same_bottom_row() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(0);
    let with_popup = render_state_to_text(&mut state, 80, 24);
    state.input_buffer = "draft".to_owned();
    state.cursor_position = 5;
    state.active_suggestion_index = None;
    let without_popup = render_state_to_text(&mut state, 80, 24);
    let input_row = |rendered: &str, needle: &str| {
        rendered
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(needle))
            .map(|(index, _)| index)
            .last()
            .expect("composer row")
    };
    assert_eq!(
        input_row(&with_popup, "› /"),
        input_row(&without_popup, "› draft")
    );
}

#[test]
fn busy_command_surfaces_stay_above_input_without_an_activity_row() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    for (width, height) in [(80, 24), (60, 18), (100, 30)] {
        for panel in [false, true] {
            let mut state = RenderState::new();
            state.status = AppStatus::Streaming;
            state.running_tools = vec!["run_command".to_owned()];
            state
                .history
                .push(ChatMessage::new("assistant", "latest transcript"));
            if panel {
                state.show_status_modal = true;
            } else {
                state.input_buffer = "/verbosity".to_owned();
                state.cursor_position = state.input_buffer.len();
                state.active_suggestion_index = Some(0);
            }
            let activity =
                super::activity_status_line(&render_snapshot(&state), false, width as usize)
                    .to_string();
            let rendered = render_state_to_text(&mut state, width, height);
            let row = |text: &str| {
                rendered
                    .lines()
                    .position(|line| line.contains(text))
                    .unwrap_or_else(|| panic!("missing {text:?}: {rendered}"))
            };
            let surface = if panel {
                "Session status"
            } else {
                "Show or set verbosity"
            };
            let composer = rendered
                .lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.contains(if panel {
                        "Ask RustCode"
                    } else {
                        "› /verbosity"
                    })
                })
                .map(|(row, _)| row)
                .last()
                .unwrap();
            assert!(
                row(surface) < composer,
                "surface must remain above composer: {rendered}"
            );
            assert!(!rendered.contains(activity.trim()), "{rendered}");
            if !panel {
                assert!(composer < row("context left"), "{rendered}");
            }
        }
    }
}

#[test]
fn context_and_settings_panels_survive_controller_tool_events_and_completion() {
    use rustcode::controller::{AgentUiEvent, SettingsPicker, ToolResult, ToolResultMetadata};
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    for settings in [false, true] {
        let mut state = RenderState::new();
        state.status = AppStatus::Streaming;
        state.show_context_modal = !settings;
        state.settings_picker = settings.then_some(SettingsPicker::Verbosity);
        state.modal_picker_index = 1;
        let mut transcript = TranscriptState::default();
        for event in [
            AgentUiEvent::TextDelta {
                text: "Streaming thought".to_owned(),
            },
            AgentUiEvent::ToolStarted {
                name: "get_time".to_owned(),
                id: "panel-call".to_owned(),
                detail: None,
            },
            AgentUiEvent::ToolFinished {
                id: "panel-call".to_owned(),
                result: ToolResult {
                    tool_name: "get_time".to_owned(),
                    content: "12:30".to_owned(),
                    diff: None,
                    file_preview: None,
                    metadata: ToolResultMetadata {
                        success: true,
                        ..Default::default()
                    },
                },
            },
            AgentUiEvent::TurnFinished {
                content: "Done".to_owned(),
                completed: true,
            },
        ] {
            transcript.apply_agent_event(&event);
            match event {
                AgentUiEvent::ToolStarted { name, .. } => state.running_tools.push(name),
                AgentUiEvent::ToolFinished { .. } => {
                    state.running_tools.clear();
                }
                AgentUiEvent::TurnFinished { .. } => state.status = AppStatus::Idle,
                _ => {}
            }
            for (width, height) in [(80, 24), (60, 18)] {
                let rendered = render_state_to_text_with_transcript(
                    &mut state,
                    &mut transcript,
                    width,
                    height,
                );
                assert!(
                    rendered.contains(if settings {
                        "Output verbosity"
                    } else {
                        "context usage"
                    }),
                    "panel vanished: {rendered}"
                );
                assert_eq!(state.modal_picker_index, 1);
            }
        }
        state.show_context_modal = false;
        state.settings_picker = None;
        let rendered = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 24);
        assert!(!rendered.contains(if settings {
            "Output verbosity"
        } else {
            "context usage"
        }));
    }
}

#[test]
fn command_output_panel_scrolls_wrapped_content_on_short_terminals() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.command_panel = Some(rustcode::controller::CommandPanel {
        title: "Help",
        content: (0..40).map(|row| format!("Unique help row {row:02} with a long description that wraps past the terminal edge")).collect::<Vec<_>>().join("\n\n"),
    });
    for (width, height) in [(40, 12), (80, 24)] {
        state.modal_scroll_row = 0;
        let start = render_state_to_text(&mut state, width, height);
        assert!(start.contains("Unique help row 00"));
        state.modal_scroll_row = 12;
        let scrolled = render_state_to_text(&mut state, width, height);
        assert!(scrolled.contains("Help"));
        assert!(!scrolled.contains("Unique help row 00"));
        assert!(scrolled.contains("Unique help row"));
        assert!(state.history.is_empty());
    }
}

#[test]
fn status_screen_uses_the_viewport_above_the_composer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.show_status_modal = true;
    state.active_session_id = "status-test".to_owned();
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("Session status"));
    assert!(rendered.contains("status-test"));
    assert!(rendered.contains("Ask RustCode to do anything"));
    assert!(
        rendered
            .lines()
            .next()
            .is_some_and(|line| line.trim().is_empty())
    );
}

#[test]
fn slash_command_modal_leaves_the_transcript_visible_above_it() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let row_of = |rendered: &str, needle: &str| {
        rendered
            .lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle:?} in {rendered:?}"))
    };

    let cases: [(fn(&mut RenderState), &str); 5] = [
        (
            |state: &mut RenderState| state.show_status_modal = true,
            "Session status",
        ),
        (
            |state: &mut RenderState| state.show_model_picker = true,
            "Select model",
        ),
        (
            |state: &mut RenderState| state.show_context_modal = true,
            "context usage",
        ),
        (
            |state: &mut RenderState| state.show_stats_modal = true,
            "Token usage",
        ),
        (
            |state: &mut RenderState| state.show_session_modal = true,
            "Session",
        ),
    ];

    for (open_modal, modal_title) in cases {
        let mut state = RenderState::new();
        state
            .history
            .push(ChatMessage::new("system", "transcript stays above"));
        open_modal(&mut state);
        let rendered = render_state_to_text(&mut state, 80, 24);
        assert!(
            row_of(&rendered, "transcript stays above") < row_of(&rendered, modal_title),
            "modal covers the transcript: {rendered:?}"
        );
    }
}

#[test]
fn stats_and_session_panels_render_their_details_without_touching_the_transcript() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.show_stats_modal = true;
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("Token usage"), "{rendered:?}");
    assert!(rendered.contains("no token data yet"), "{rendered:?}");
    assert!(rendered.contains("esc to close"), "{rendered:?}");

    let mut state = RenderState::new();
    state.show_session_modal = true;
    state.active_session_id = "session-test".to_owned();
    state
        .history
        .push(ChatMessage::new("user", "one user turn"));
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("session-test"), "{rendered:?}");
    assert!(rendered.contains("1 user"), "{rendered:?}");
    assert_eq!(
        rendered.matches("one user turn").count(),
        1,
        "the panel summarises the turn instead of repeating it: {rendered:?}"
    );
}

#[test]
fn session_panel_text_can_be_selected_and_copied_without_panel_padding() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{backend::TestBackend, style::Modifier};

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.show_session_modal = true;
    state.active_session_id = "session-test".to_owned();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();

    let area = transcript
        .panel_selection_area
        .expect("session panel exposes its body to selection");
    let buffer = terminal.backend().buffer();
    let (column, row) = (area.y..area.bottom())
        .find_map(|row| {
            let line = (area.x..area.right())
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>();
            line.find("session-test")
                .map(|offset| (area.x + offset as u16, row))
        })
        .expect("the selected body contains the session id");
    let end_column = column + "session-test".len() as u16 - 1;
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: end_column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    transcript.panel_selection.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: end_column,
        row,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(
        transcript.panel_selection.selected_text().as_deref(),
        Some("session-test")
    );
    assert!(
        !transcript.selection.has_selection(),
        "panel selection must remain independent from transcript selection"
    );
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    assert!(
        terminal.backend().buffer()[(column, row)]
            .modifier
            .contains(Modifier::REVERSED),
        "the selected panel text is visibly highlighted"
    );
    state.show_session_modal = false;
    state.show_status_modal = true;
    let previous_area = area;
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    assert_eq!(transcript.panel_selection_area, Some(previous_area));
    assert!(
        !transcript.panel_selection.has_selection(),
        "a same-sized status panel must not inherit the session panel range"
    );

    state.show_status_modal = false;
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    assert!(transcript.panel_selection_area.is_none());
    assert!(!transcript.panel_selection.has_selection());
}

#[test]
fn info_command_panel_text_can_be_selected_and_copied() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.command_panel = Some(rustcode::controller::CommandPanel {
        title: "About RustCode",
        content: "Session: session-test\nModel: café 🙂\nTurn: inactive\nQueue: 0".into(),
    });
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();

    let area = transcript
        .panel_selection_area
        .expect("generic info panel exposes its body to selection");
    let buffer = terminal.backend().buffer();
    let (column, row) = (area.y..area.bottom())
        .find_map(|row| {
            let line = (area.x..area.right())
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>();
            line.find("café 🙂")
                .map(|offset| (area.x + offset as u16, row))
        })
        .expect("the info body contains accented and wide text");
    let selected = "café 🙂";
    let end_column = column + selected.width() as u16 - 1;
    for (kind, column) in [
        (MouseEventKind::Down(MouseButton::Left), column),
        (MouseEventKind::Drag(MouseButton::Left), end_column),
        (MouseEventKind::Up(MouseButton::Left), end_column),
    ] {
        transcript.panel_selection.mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert_eq!(
        transcript.panel_selection.selected_text().as_deref(),
        Some(selected)
    );
}

#[test]
fn inline_command_selection_is_distinct_from_typed_input() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, style::Modifier};

    for input in ["/mo", "/model"] {
        let mut state = RenderState::new();
        state.input_buffer = input.to_owned();
        state.cursor_position = input.len();
        state.active_suggestion_index = Some(0);
        state.history.push(rustcode::controller::ChatMessage::new(
            "user",
            "existing conversation",
        ));

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &mut state);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row_text = |row: u16| {
            (0..100)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        };

        let input_row = (0..20)
            .rev()
            .find(|row| row_text(*row).contains(input))
            .expect("composer input row should be visible");
        let input_column = row_text(input_row)
            .find(input)
            .expect("typed command should be visible") as u16;
        assert_eq!(
            buffer[(input_column, input_row)].fg,
            COLOR_TEXT(),
            "typed slash command should use the default text color"
        );
        assert!(
            !buffer[(input_column, input_row)]
                .modifier
                .contains(Modifier::BOLD),
            "typed slash command should use normal weight"
        );

        let filtered_cmds = rustcode::controller::filtered_commands(input);
        let snapshot = render_snapshot(&state);
        let mut popup_terminal = Terminal::new(TestBackend::new(100, 2)).unwrap();
        popup_terminal
            .draw(|frame| {
                super::modals::render_popup_menu(
                    frame,
                    &snapshot,
                    &filtered_cmds,
                    ratatui::layout::Rect::new(0, 0, 100, 2),
                );
            })
            .unwrap();
        let selected_cell = &popup_terminal.backend().buffer()[(2, 0)];
        assert_eq!(selected_cell.fg, ratatui::style::Color::Black);
        assert_eq!(selected_cell.bg, COLOR_PRIMARY());
        assert!(selected_cell.modifier.contains(Modifier::BOLD));
        assert_eq!(popup_terminal.backend().buffer()[(0, 0)].symbol(), "›");
    }
}

#[test]
fn inline_command_recommendations_style_unselected_rows_as_default_text() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, style::Modifier};

    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(1);

    let filtered_cmds = rustcode::controller::filtered_commands(&state.input_buffer);
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(100, 2)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_popup_menu(
                frame,
                &snapshot,
                &filtered_cmds,
                ratatui::layout::Rect::new(0, 0, 100, 2),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let unselected_cell = &buffer[(2, 0)];
    assert_eq!(unselected_cell.fg, COLOR_TEXT());
    assert_eq!(
        unselected_cell.bg,
        COLOR_PANEL(),
        "unselected rows should sit on the panel background, not the transcript"
    );
    assert!(!unselected_cell.modifier.contains(Modifier::BOLD));
    assert_eq!(
        unselected_cell.symbol(),
        filtered_cmds[0].name[0..1].to_owned()
    );
}

#[test]
fn inline_command_popup_marks_selection_and_clips_descriptions_to_width() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, layout::Rect};

    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.active_suggestion_index = Some(0);
    let commands = rustcode::controller::filtered_commands("/");
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(32, 1)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_popup_menu(frame, &snapshot, &commands, Rect::new(0, 0, 32, 1));
        })
        .unwrap();

    let row = (0..32)
        .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
        .collect::<String>();
    assert!(row.starts_with("› /cancel"), "rendered: {row:?}");
    assert!(
        row.contains('…'),
        "long descriptions should be clipped: {row:?}"
    );
}

/// The popup finds a command the user mistyped and marks the characters that
/// matched, so a fuzzy hit is not a mystery (#1588).
#[test]
fn inline_command_popup_matches_fuzzily_and_marks_the_match() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::{backend::TestBackend, layout::Rect};

    // A transposed command name: neither an exact nor a prefix match, so it
    // matched nothing before.
    let mut state = RenderState::new();
    state.input_buffer = "/modle".to_owned();
    state.cursor_position = state.input_buffer.len();
    state.active_suggestion_index = Some(0);
    let commands = rustcode::controller::filtered_commands(&state.input_buffer);
    assert_eq!(
        commands.first().map(|command| command.name),
        Some("/model"),
        "the typo must still find the command"
    );
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(60, 2)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_popup_menu(frame, &snapshot, &commands, Rect::new(0, 0, 60, 2));
        })
        .unwrap();

    let row = (0..60)
        .map(|column| terminal.backend().buffer()[(column, 0)].symbol())
        .collect::<String>();
    assert!(row.contains("/model"), "rendered: {row:?}");

    // A longer prefix keeps several rows, so the unselected ones carry the
    // match marking.
    let mut state = RenderState::new();
    state.input_buffer = "/mo".to_owned();
    state.cursor_position = state.input_buffer.len();
    state.active_suggestion_index = Some(0);
    let commands = rustcode::controller::filtered_commands(&state.input_buffer);
    let snapshot = render_snapshot(&state);
    let mut terminal = Terminal::new(TestBackend::new(60, 3)).unwrap();
    terminal
        .draw(|frame| {
            super::modals::render_popup_menu(frame, &snapshot, &commands, Rect::new(0, 0, 60, 3));
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let mut marked = String::new();
    for row in 0..3 {
        if buffer[(0, row)].symbol() == "›" {
            continue;
        }
        marked.extend(
            (0..60)
                .filter(|column| buffer[(*column, row)].fg == COLOR_PRIMARY())
                .map(|column| buffer[(column, row)].symbol().to_owned()),
        );
    }
    assert!(
        marked.contains("mo"),
        "the matched characters must be marked on unselected rows: {marked:?}"
    );
}

#[test]
fn welcome_banner_renders_without_a_conversation() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();

    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(
        rendered.contains("model:")
            && rendered.contains("effort:")
            && rendered.contains("context:")
            && rendered.contains("OS sandbox:")
            && rendered.contains("directory:"),
        "the empty chat must display its welcome banner: {rendered:?}"
    );
    assert!(
        rendered.contains(">_ RustCode"),
        "the welcome banner header must include '>_ RustCode': {rendered:?}"
    );
    assert!(
        rendered.contains("session:") && rendered.contains(&state.active_session_id),
        "the welcome banner must include the active session ID: {rendered:?}"
    );
    assert!(
        rendered.contains("branch:")
            && rendered.contains("/model")
            && rendered.contains("/effort")
            && rendered.contains("/context")
            && rendered.contains("help:")
            && rendered.contains("/help"),
        "the welcome banner must include branch and help rows: {rendered:?}"
    );
}

/// The effective mode must be readable from the banner and the status line
/// without opening anything, otherwise the unrestricted mode exists but cannot
/// be noticed (#1540).
#[test]
fn welcome_banner_names_the_effective_sandbox_mode() {
    for (mode, expected) in [
        (
            rustcode::controller::SandboxMode::WorkspaceWrite,
            "workspace/session writes; no network",
        ),
        (
            rustcode::controller::SandboxMode::Trusted,
            "trusted process permissions; no OS sandbox",
        ),
    ] {
        let mut state = RenderState::new();
        state.config.sandbox_mode = mode;

        let rendered = super::build_claude_startup_banner(&state, 100, 28)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("OS sandbox:"),
            "the banner must always show the mode: {rendered:?}"
        );
        assert!(
            rendered.contains(expected),
            "{mode} must render as {expected:?}: {rendered:?}"
        );
    }
}

#[test]
fn welcome_banner_shows_yolo_effective_permissions_instead_of_saved_restrictions() {
    let mut state = RenderState::new();
    state.config.sandbox_mode = rustcode::controller::SandboxMode::ReadOnly;
    state.auto_confirm = true;
    let rendered = super::build_claude_startup_banner(&state, 100, 28)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("trusted process permissions; no OS sandbox"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("read-only host; no network"),
        "{rendered}"
    );
}

#[test]
fn welcome_banner_shows_active_model_effort_and_context_window() {
    let mut state = RenderState::new();
    state.api_base_url = "http://localhost/test".to_string();
    state.model_name = "test-model".to_string();
    // The engine resolves the active profile and window from `config.models`
    // into the view (`controller::render_state`); seed both here.
    state.config.models = vec![rustcode::controller::ModelProfile {
        name: "test-profile".to_string(),
        url: state.api_base_url.clone(),
        model: state.model_name.clone(),
        context_window: Some(128_000),
        reasoning_effort: Some("high".to_string()),
        ..Default::default()
    }];
    state.active_model_profile = Some(state.config.models[0].clone());
    state.active_context_window = 128_000;

    let lines = super::build_claude_startup_banner(&state, 100, 28);
    let rendered = lines
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("effort:      high"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("context:     128.0K tokens"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("/context to change"));
}

#[test]
fn welcome_banner_pads_above_session_and_groups_session_with_model() {
    let state = RenderState::new();
    let lines = super::build_claude_startup_banner(&state, 100, 28);
    let rendered: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    let session = rendered
        .iter()
        .position(|line| line.contains("session:"))
        .expect("banner has a session row");
    let model = rendered
        .iter()
        .position(|line| line.contains("model:"))
        .expect("banner has a model row");
    // Blank padding between the top border and session.
    assert!(
        session >= 2 && rendered[session - 1].trim_matches(['│', ' ']).is_empty(),
        "expected blank padding above session: {rendered:?}"
    );
    // No gap between session and model.
    assert_eq!(
        model,
        session + 1,
        "session and model must be adjacent: {rendered:?}"
    );
}

#[test]
fn welcome_wordmark_has_room_above_and_to_its_left() {
    let state = RenderState::new();
    let rendered = super::build_claude_startup_banner(&state, 100, 28)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let first_wordmark = rendered
        .iter()
        .position(|line| line.contains('█'))
        .expect("wordmark is visible");
    assert!(first_wordmark >= 3);
    assert!(
        rendered[1..first_wordmark]
            .iter()
            .all(|line| line.trim_matches(['│', ' ']).is_empty())
    );
    assert!(rendered[first_wordmark].starts_with("│    "));
}

#[test]
fn welcome_wordmark_colors_the_whole_c_white() {
    let state = RenderState::new();
    let lines = super::build_claude_startup_banner(&state, 100, 28);
    let glyph_row = lines
        .iter()
        .find(|line| line.to_string().contains("▄▀▀▀ █   █"))
        .expect("wordmark glyph row is visible");
    assert_eq!(
        glyph_row.spans[2].style.fg,
        Some(ratatui::style::Color::Rgb(181, 139, 255))
    );
    assert_eq!(
        glyph_row.spans[3].style.fg,
        Some(ratatui::style::Color::White)
    );
    assert_eq!(glyph_row.spans[3].content.chars().next(), Some('▄'));
    assert!(glyph_row.spans[3].content.starts_with("▄▀▀▀▀"));
}

#[test]
fn welcome_banner_places_hints_beside_values_and_help_on_its_own_row() {
    let state = RenderState::new();
    let lines = super::build_claude_startup_banner(&state, 100, 28);
    let rendered: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    for (value, command) in [
        ("model:", "/model to change"),
        ("effort:", "/effort to change"),
        ("context:", "/context to change"),
    ] {
        let row = rendered
            .iter()
            .find(|line| line.contains(value))
            .expect("banner value row");
        assert!(row.contains(command), "{command} missing: {rendered:?}");
    }
    let help_row = rendered
        .iter()
        .find(|line| line.contains("help:"))
        .expect("banner help row");
    assert!(help_row.contains("run this command to get help: /help"));

    let hint_positions = ["/model", "/effort", "/context"]
        .into_iter()
        .map(|command| {
            rendered
                .iter()
                .find_map(|line| line.find(command))
                .unwrap_or_else(|| panic!("{command} missing: {rendered:?}"))
        })
        .collect::<Vec<_>>();
    assert!(
        hint_positions
            .iter()
            .all(|position| *position == hint_positions[0]),
        "slash-command hints must share a second column: {rendered:?}"
    );

    let session_row = rendered
        .iter()
        .find(|line| line.contains("session:"))
        .expect("banner session row");
    assert_eq!(
        session_row.chars().position(|character| character == 's'),
        Some(5),
        "welcome content should have comfortable left padding: {rendered:?}"
    );
}

#[test]
fn welcome_banner_includes_padding_below() {
    let state = RenderState::new();
    let lines = super::render_live_tail(&state, 100, 28);
    assert!(!lines.is_empty());
    // The last line should be empty padding below the banner box
    let last = &lines[lines.len() - 1];
    assert!(
        last.spans.is_empty() || last.spans.iter().all(|s| s.content.trim().is_empty()),
        "welcome banner must end with a blank padding line"
    );
}

#[test]
fn welcome_banner_includes_padding_before_bottom_border() {
    let state = RenderState::new();
    let lines = super::render_live_tail(&state, 100, 28);
    let bottom_border = lines
        .iter()
        .position(|line| line.to_string().contains('╰'))
        .expect("welcome banner must include a bottom border");
    assert!(bottom_border > 0);
    assert!(
        lines[bottom_border - 1]
            .to_string()
            .trim_matches('│')
            .trim()
            .is_empty(),
        "welcome banner must have a blank line before its bottom border"
    );
}

#[test]
fn welcome_banner_adapts_to_small_viewports_without_truncating_box() {
    let state = RenderState::new();
    // Test with small height = 6
    let lines = super::render_live_tail(&state, 100, 6);
    let text = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("model:"));
    assert!(
        text.contains("╰"),
        "banner must end cleanly with a bottom border"
    );
}

#[test]
fn welcome_banner_stays_inside_narrow_terminal_width() {
    let state = RenderState::new();
    for width in [8, 16, 32] {
        let lines = super::build_claude_startup_banner(&state, width, 28);

        assert!(!lines.is_empty());
        assert!(
            lines.iter().all(|line| line.width() <= width),
            "welcome banner must not overflow a narrow terminal: {lines:?}"
        );
    }
}

#[test]
fn welcome_banner_omits_hints_that_do_not_fit() {
    let state = RenderState::new();
    let lines = super::build_claude_startup_banner(&state, 32, 28);
    let rendered: Vec<String> = lines.iter().map(|line| line.to_string()).collect();

    assert!(lines.iter().all(|line| line.width() <= 32));
    assert!(rendered.iter().all(|line| !line.contains("to change")));
    let help_row = rendered
        .iter()
        .find(|line| line.contains("help:"))
        .expect("narrow banner keeps a help row");
    assert!(help_row.contains("/help"));
}

#[test]
fn first_message_keeps_the_welcome_cell_in_the_transcript() {
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("user", "hello from the first turn"));

    let snapshot = super::render_snapshot::render_snapshot(&state);
    let mut transcript = TranscriptState::default();
    let lines =
        super::render_visible_conversation_with_transcript(&snapshot, 100, 28, &mut transcript);
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("directory:")),
        "the welcome banner should remain above the first message"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("hello from the first turn"))
    );

    assert_eq!(super::desired_height(&state, &mut transcript, 100, 40), 40);
}

#[test]
fn welcome_cell_remains_reachable_after_many_messages() {
    let mut state = RenderState::new();
    for index in 0..40 {
        state
            .history
            .push(ChatMessage::new("user", format!("message {index}")));
    }
    let snapshot = super::render_snapshot::render_snapshot(&state);
    let mut transcript = TranscriptState::default();
    transcript.scroll_up(10_000);
    let lines =
        super::render_visible_conversation_with_transcript(&snapshot, 80, 20, &mut transcript);
    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("directory:")),
        "scrolling to the top should reach the welcome cell"
    );
}

#[test]
fn queue_preview_shows_recent_user_prompts_without_wakeups() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.pending_queue = vec![
        "first prompt".to_owned(),
        "second prompt".to_owned(),
        "third prompt".to_owned(),
        "fourth prompt".to_owned(),
        "__task_wakeup__:task-123".to_owned(),
    ];

    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(rendered.contains("queued follow-ups (4) · ↑ edit last"));
    assert!(rendered.contains("second prompt"));
    assert!(rendered.contains("third prompt"));
    assert!(rendered.contains("fourth prompt"));
    assert!(!rendered.contains("first prompt"));
    assert!(!rendered.contains("__task_wakeup__"));
}

#[test]
fn steering_previews_are_separate_and_show_interrupt_and_mode_hints() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.status = rustcode::controller::AppStatus::Streaming;
    state.steering_interruptible = true;
    state.steering_escape_will_interrupt = true;
    state.pending_steers = vec!["first steer".to_owned(), "second steer".to_owned()];
    state.pending_queue = vec!["follow-up one".to_owned(), "follow-up two".to_owned()];
    state.input_buffer = "draft text".to_owned();

    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();

    assert!(
        rendered.contains("pending steers · apply after next tool result or when the turn ends")
    );
    assert!(rendered.find("first steer").unwrap() < rendered.find("second steer").unwrap());
    assert!(rendered.contains("queued follow-ups (2) · ↑ edit last"));
    assert!(rendered.contains("follow-up one"));
    assert!(rendered.contains("follow-up two"));
    assert!(!rendered.contains("esc interrupt and apply now"));
    assert!(!rendered.contains("Working · "));
    assert!(!rendered.contains("Steer · Tab switches to Queue"));
}

#[test]
fn steering_escape_hint_is_hidden_when_escape_dismisses_completion_or_selection() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    for blocker in ["completion", "selection"] {
        let mut state = RenderState::new();
        state.status = rustcode::controller::AppStatus::Streaming;
        state.steering_interruptible = true;
        state.pending_steers.push("apply this steer".to_owned());
        match blocker {
            // A completion or a transcript selection takes Escape first, so
            // the engine resolves the interrupt hint to false.
            "completion" => {
                state.input_buffer = "/mo".to_owned();
                state.steering_escape_will_interrupt = false;
            }
            "selection" => {
                state.input_buffer = "draft text".to_owned();
                state.steering_escape_will_interrupt = false;
            }
            _ => unreachable!(),
        }

        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &mut state);
            })
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            !rendered.contains("esc interrupt and apply now"),
            "Escape should first handle the {blocker}"
        );
        assert!(
            !rendered.contains("esc interrupt"),
            "do not imply Escape will interrupt while it handles the {blocker}"
        );
    }
}

/// The footer names the copy chord while a transcript selection is live, and
/// stops naming it the moment the selection goes away (#1542).
///
/// The sibling of the Escape test above: Escape is claimed by the selection, so
/// the footer describes the selection rather than the copy it has not made yet.
#[test]
fn selection_copy_hint_appears_with_the_selection_and_leaves_with_it() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let binding = rustcode::controller::copy_selection_binding();
    let mut state = RenderState::new();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();

    let footer = |terminal: &Terminal<TestBackend>| {
        let buffer = terminal.backend().buffer();
        (0..100)
            .map(|column| buffer[(column, footer_row(20))].symbol())
            .collect::<String>()
    };

    // No selection: the footer keeps naming the session, not a copy key.
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let idle = footer(&terminal);
    assert!(!idle.contains(binding), "idle footer: {idle:?}");
    assert!(!idle.contains("or right-click"), "idle footer: {idle:?}");
    assert!(
        idle.contains(&state.model_name),
        "idle footer should keep the session metadata: {idle:?}"
    );

    // A released selection: the key that copies it is named outright.
    select_transcript_text(&mut state, &mut transcript, (2, 2), (20, 2));
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let selected = footer(&terminal);
    assert!(
        selected.starts_with(&format!("  {binding} · or right-click")),
        "selected footer: {selected:?}"
    );
    assert!(
        !selected.contains(&state.model_name),
        "the hint replaces the session metadata: {selected:?}"
    );

    // Escape clears the selection -- `selection_owns_key` routes Esc to it
    // before the composer or the turn ever sees the key -- and the footer gives
    // the row straight back. Nothing was copied here, so the hint was tracking
    // the gesture rather than the copy.
    transcript.selection.clear();
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let cleared = footer(&terminal);
    assert!(!cleared.contains(binding), "cleared footer: {cleared:?}");
    assert!(
        cleared.contains(&state.model_name),
        "cleared footer should fall back to the session metadata: {cleared:?}"
    );
}

/// Copy feedback is the answer to the key that was just pressed, so it holds
/// the footer while it lasts and the hint returns behind it (#1542).
#[test]
#[test]
fn a_panel_still_shows_the_copy_notice_in_the_footer() {
    // Marking text inside an open slash-command panel used to hide the footer
    // entirely, so the copy result was never reported.
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.input_buffer = "/ver".to_owned();
    state.show_command_picker = true;
    assert!(
        !super::composer_footer_visible(&render_snapshot(&state)),
        "a panel hides the standing metadata by default"
    );

    state.transient_notice = Some("Copied selection to clipboard".to_owned());
    assert!(
        super::composer_footer_visible(&render_snapshot(&state)),
        "a one-shot notice must survive an open panel"
    );

    let rendered = render_state_to_text(&mut state, 90, 24);
    assert!(
        rendered.contains("Copied selection to clipboard"),
        "{rendered}"
    );
}

#[test]
fn copy_feedback_notice_holds_the_footer_until_it_expires() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    let binding = rustcode::controller::copy_selection_binding();
    let hint = footer_row_with_transcript_selection(&mut state, 100, 20);
    assert!(hint.contains(binding), "hint before the copy: {hint:?}");

    state.transient_notice = Some("Copied selection to clipboard".to_owned());
    let noticed = footer_row_with_transcript_selection(&mut state, 100, 20);
    assert!(
        noticed.contains("Copied selection to clipboard"),
        "the copy result must not be replaced by the copy key: {noticed:?}"
    );
    assert!(
        !noticed.contains(binding),
        "the hint stands down while its own result is on screen: {noticed:?}"
    );

    state.transient_notice = None;
    let resumed = footer_row_with_transcript_selection(&mut state, 100, 20);
    assert!(
        resumed.contains(binding),
        "the selection outlives the notice, so the hint returns: {resumed:?}"
    );
}

// Regression: the tool-result cache used to `clear()` the whole map at the
// cap, throwing away every still-visible result and forcing a full
// re-render on the next frame. It now drops a single cold entry.
#[test]
fn tool_result_cache_evicts_one_lru_entry_at_cap() {
    use super::{TOOL_RESULT_CACHE_CAP, lru::LruCache, tool_transcript::cached_tool_result_in};

    let cap = TOOL_RESULT_CACHE_CAP;
    let cache = std::cell::RefCell::new(LruCache::new(cap));
    for i in 0..cap {
        cached_tool_result_in(&cache, i as u64, || vec![Line::from(format!("result {i}"))]);
    }
    assert_eq!(cache.borrow().entries.len(), cap);

    // Read the oldest entry so it becomes the most recently used one; a hit
    // must refresh recency.
    let oldest = 0;
    cached_tool_result_in(&cache, oldest, || panic!("cache hit must not render"));

    // Exceed the cap by one: exactly one entry is evicted, and it is the
    // least recently used one rather than the entry just read.
    let overflow = cap as u64;
    cached_tool_result_in(&cache, overflow, || vec![Line::from("overflow")]);
    let cache = cache.borrow();
    assert_eq!(cache.entries.len(), cap, "cap must hold after overflow");
    assert!(
        cache.entries.contains_key(&oldest),
        "entry read just before the insert must survive"
    );
    assert!(
        !cache.entries.contains_key(&1),
        "the least recently used entry is the eviction victim"
    );
    assert!(
        cache.entries.contains_key(&overflow),
        "the new entry must be cached"
    );
}

#[test]
fn theme_change_changes_cache_keys() {
    use super::{theme, tool_result_cache_key};

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let verbosity = rustcode::controller::Verbosity::Low;
    theme::set_active_theme("default");
    let key1 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);
    theme::set_active_theme("nord");
    let key2 = tool_result_cache_key("Bash", "result 0", 80, &verbosity, false);

    assert_ne!(
        key1, key2,
        "cache key must differ when active theme changes"
    );
    theme::set_active_theme("default");
}

#[test]
fn sky_theme_loads_and_updates_syntax_highlighting() {
    use super::theme;

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let sky = theme::get_palette("sky");
    assert_eq!(sky.name, "sky");
    assert_eq!(sky.primary, ratatui::style::Color::Rgb(56, 148, 240));
    assert_eq!(sky.secondary, ratatui::style::Color::Rgb(136, 196, 56));
    assert_eq!(sky.panel, ratatui::style::Color::Rgb(22, 32, 50));

    theme::set_active_theme("sky");
    assert_eq!(
        super::COLOR_PRIMARY(),
        ratatui::style::Color::Rgb(56, 148, 240)
    );
    assert_eq!(
        super::COLOR_SECONDARY(),
        ratatui::style::Color::Rgb(136, 196, 56)
    );

    let spans = super::highlight_code_line("let x = 42;", "rust", false);
    assert!(!spans.is_empty());

    // Restore default theme
    theme::set_active_theme("default");
}

#[test]
fn custom_tools_render_pascalcase_with_param() {
    use super::{format_pi_tool_action, to_pascal_case};

    assert_eq!(to_pascal_case("use_skill"), "UseSkill");
    assert_eq!(to_pascal_case("complete_task"), "CompleteTask");
    assert_eq!(to_pascal_case("git-feature-workflow"), "GitFeatureWorkflow");

    let (label, arg) = format_pi_tool_action(
        "use_skill",
        &serde_json::json!({"name": "git-feature-workflow"}),
        None,
    );
    assert_eq!(label, "UseSkill");
    assert_eq!(arg, "git-feature-workflow");

    let (label, arg) = format_pi_tool_action(
        "complete_task",
        &serde_json::json!({"result": "done"}),
        None,
    );
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "result=\"done\"");

    let (label, arg) = format_pi_tool_action("complete_task", &serde_json::json!({}), None);
    assert_eq!(label, "CompleteTask");
    assert_eq!(arg, "");

    // Built-in aliases are unchanged.
    let (label, _) =
        format_pi_tool_action("run_command", &serde_json::json!({"command": "ls"}), None);
    assert_eq!(label, "Bash");
}

#[test]
fn mcp_tools_render_with_server_and_tool_name() {
    let (label, arg) = super::format_pi_tool_action(
        "mcp__mail_mcp__SearchEmails",
        &serde_json::json!({"query": "*"}),
        None,
    );
    assert_eq!(label, "mail_mcp.SearchEmails");
    assert_eq!(arg, "query=\"*\"");
}

#[test]
fn tool_path_formatting_uses_captured_snapshot_home() {
    let state = RenderState::new();
    let snapshot = render_snapshot(&state);
    let Some(home) = snapshot.home_path() else {
        return;
    };
    let path = format!("{home}/project/file.rs");

    let (_, rendered) = format_pi_tool_action(
        "view_file",
        &serde_json::json!({"path": path}),
        snapshot.home_path(),
    );

    assert_eq!(rendered, "~/project/file.rs");
}

#[test]
fn committed_exploration_summary_matches_live_safe_parameters() {
    let args = serde_json::json!({
        "path": "/workspace/src/lib.rs",
        "start_line": 10,
        "end_line": 20,
        "content": "secret source must not be shown"
    });
    let (_, rendered) = super::format_pi_tool_action("view_file", &args, Some("/workspace"));
    assert_eq!(rendered, "~/src/lib.rs (lines 10-20)");
    assert!(!rendered.contains("secret"));
}

#[test]
fn persisted_edit_result_resolves_tool_name_without_previous_call() {
    let result = "replace_file_content: successfully replaced target_content\n\n```diff\n@@ -1 +1 @@\n-old\n+new\n```";
    let tool_name = super::resolve_tool_result_name(None, Some("replace_file_content"), result);

    assert_eq!(tool_name.as_deref(), Some("replace_file_content"));
    assert!(
        super::render_tool_result(
            tool_name.as_deref().unwrap(),
            result.strip_prefix("replace_file_content: ").unwrap(),
            80,
            &rustcode::controller::Verbosity::Low,
            false,
        )
        .iter()
        .any(|line| line.spans.iter().any(|span| span.content.contains("new")))
    );
}

#[test]
fn committed_tool_result_shows_action_status_and_indented_output() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\n504 passed")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: Some(0),
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("• Ran $ cargo test · exit 0"))
    );
    assert!(
        rendered.iter().any(|line| line.contains("504 passed")),
        "command output must be rendered beneath its header: {rendered:?}"
    );
}

#[test]
fn committed_tool_result_shows_failure_status() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "run_command: exit code: 1\nstderr:\npermission denied",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            arguments_hash: String::new(),
            success: false,
            exit_code: Some(1),
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("• Ran $ cargo test · exit 1"))
    );
}

#[test]
fn ask_question_renders_prompt_and_answer_in_committed_history() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "ask_question".to_owned(),
            arguments: r#"{"question":"Where should the version data come from?","options":["CHANGELOG.md","Releases API"]}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "ask_question: User selected: Releases API is the source, and it should include all data from the organization's release record without synthesizing a separate summary; preserve the full response.",
        )
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "ask_question".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    // The question/answer pair must survive as a transcript entry: hiding it
    // left users with no trace of what they were asked or what they chose.
    let entry = super::tool_transcript_entry(&render_snapshot(&state), 1, 80, false)
        .expect("ask_question must not be hidden from the transcript");
    assert_eq!(entry.action, "Asked");
    assert!(
        entry
            .target
            .contains("Where should the version data come from?"),
        "question missing from headline: {}",
        entry.target
    );
    assert!(
        entry.target.contains("Releases API"),
        "answer missing from headline: {}",
        entry.target
    );
    let full_answer = entry
        .body
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        full_answer.contains("preserve the full response"),
        "expanded body retains the full human answer: {full_answer:?}"
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered.iter().any(|line| line.contains("Asked")
            && line.contains("Where should the version data come from?")),
        "question headline missing: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("Releases API")),
        "answer missing from transcript: {rendered:?}"
    );
}

#[test]
fn ask_question_cancellation_renders_visibly() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "ask_question".to_owned(),
            arguments: r#"{"question":"Proceed?","options":["Yes","No"]}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "ask_question: User cancelled or provided no selection.",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "ask_question".to_owned(),
            arguments_hash: String::new(),
            success: false,
            exit_code: None,
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let entry = super::tool_transcript_entry(&render_snapshot(&state), 1, 80, false)
        .expect("cancelled ask_question must stay visible");
    assert!(
        entry.target.contains("Proceed?"),
        "question missing after cancellation: {}",
        entry.target
    );
    assert!(!entry.success, "cancellation must not render as success");
}

#[test]
fn chained_ask_question_renders_count_and_every_answer() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "ask_question".to_owned(),
            arguments: r#"{"questions": [{"header": "Source", "question": "Where from?", "options": ["A", "B"]}, {"header": "Count", "question": "How many?", "options": ["1", "2"]}]}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "ask_question: User answers:\n[Source] Where from? → A\n[Count] How many? → 2",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "ask_question".to_owned(),
            arguments_hash: String::new(),
            success: true,
            exit_code: None,
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let entry = super::tool_transcript_entry(&render_snapshot(&state), 1, 80, false)
        .expect("chained ask_question must stay visible");
    assert_eq!(entry.action, "Asked");
    assert!(
        entry.target.contains("2 questions"),
        "chain count missing: {}",
        entry.target
    );
    assert!(
        entry.target.contains("Where from?"),
        "first answer missing: {}",
        entry.target
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    // The headline may wrap across rows; check the count and the answers on
    // the joined transcript.
    assert!(
        rendered.iter().any(|line| line.contains("2 questions")),
        "chain count missing: {rendered:?}"
    );
    let joined = rendered.join("\n");
    assert!(
        joined.contains("Where from?") && joined.contains("How many?"),
        "chain answers missing: {rendered:?}"
    );
}

#[test]
fn use_skill_renders_in_committed_history() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "use_skill".to_owned(),
            arguments: r#"{"name":"clockify"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "use_skill: <skill_content>...</skill_content>")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "use_skill".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Ran");
    assert!(rendered.iter().any(|line| line == "  └ UseSkill clockify"));
}

#[test]
fn incremental_tool_round_continuation_has_no_second_group_heading() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.extend([
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "view_file".to_owned(),
            arguments: r#"{"TargetFile":"index.html"}"#.to_owned(),
        }]),
        ChatMessage::new("tool", "view_file: first read")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "view_file".to_owned(),
                success: true,
                ..Default::default()
            }),
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-2".to_owned(),
            name: "view_file".to_owned(),
            arguments: r#"{"TargetFile":"js/app.js"}"#.to_owned(),
        }]),
        ChatMessage::new("tool", "view_file: second read")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "view_file".to_owned(),
                success: true,
                ..Default::default()
            }),
    ]);

    let snapshot = render_snapshot(&state);
    let continuation =
        super::render_committed_tool_result_continuation_snapshot(&snapshot, &[3], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

    assert!(continuation.iter().any(|line| line.contains("Read")));
    assert!(!continuation.iter().any(|line| line.contains("Explored")));
}

#[test]
fn high_verbosity_keeps_tool_call_summaries_visible() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "use_skill".to_owned(),
            arguments: r#"{"name":"clockify"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "use_skill: loaded clockify")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "use_skill".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Ran", "  └ UseSkill clockify"]);
}

#[test]
fn completed_generic_tool_uses_ran_heading_and_indented_child() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "get_time".to_owned(),
            arguments: "{}".to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Ran", "  └ GetTime"]);
}

#[test]
fn high_verbosity_batches_consecutive_commands_under_one_heading() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"cargo check --tests"}"#.to_owned(),
            },
        ]));
    for (id, command) in [
        ("call-1", "git status --short"),
        ("call-2", "cargo check --tests"),
    ] {
        state.history.push(
            ChatMessage::new(
                "tool",
                format!("run_command: exit code: 0\n{command} output"),
            )
            .answering(Some(id.to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Ran",
            "  └ Bash git status --short",
            "    Bash cargo check --tests"
        ]
    );
    assert!(!rendered.iter().any(|line| line.contains("output")));
}

#[test]
fn high_verbosity_keeps_mixed_provider_batch_under_one_ran_heading() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        ["• Ran", "  └ GetTime", "    Bash git status --short"]
    );
}

#[test]
fn worked_separator_only_labels_concrete_work_over_one_minute() {
    use rustcode::controller::{ChatMessage, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "fix it"));
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0").with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            ..Default::default()
        }),
    );
    let mut assistant = ChatMessage::new("assistant", "Done.");
    assistant.response_time_ms = Some(125_000);
    state.history.push(assistant);

    let separator = super::render_work_separator_before_assistant(&state, 2, 80);
    assert_eq!(separator.len(), 2);
    assert!(
        separator[0]
            .to_string()
            .starts_with("─ Worked for 2m 05s ─")
    );
    assert!(separator[1].to_string().is_empty());

    state.history[2].response_time_ms = Some(12_000);
    assert_eq!(
        super::render_work_separator_before_assistant(&state, 2, 12)[0].to_string(),
        "────────────"
    );
    assert!(super::render_work_separator_before_assistant(&state, 0, 80).is_empty());
}

#[test]
fn work_separator_follows_tool_with_padding_gap() {
    use rustcode::controller::{ChatMessage, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "explore"));
    state.history.push(
        ChatMessage::new("tool", "view_file: read main.rs").with_tool_result(ToolResultRecord {
            tool_name: "view_file".to_owned(),
            success: true,
            ..Default::default()
        }),
    );
    let mut assistant = ChatMessage::new("assistant", "Found it.");
    assistant.response_time_ms = Some(254_000);
    state.history.push(assistant);

    let separator = super::render_work_separator_before_assistant(&state, 2, 80);
    assert_eq!(separator.len(), 2);
    assert!(
        separator[0]
            .to_string()
            .starts_with("─ Worked for 4m 14s ─")
    );
    assert_eq!(separator[1].to_string(), "");
}

#[test]
fn high_verbosity_hides_generic_tool_details() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "mcp_custom_tool: completed\nline 1\nline 2")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                arguments_hash: String::new(),
                success: true,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Ran");
    assert!(
        rendered
            .iter()
            .any(|line| line.starts_with("  └ McpCustomTool"))
    );
    assert!(rendered.iter().any(|line| line.contains("McpCustomTool")));
    assert!(!rendered.iter().any(|line| line.contains("completed")));
    assert!(!rendered.iter().any(|line| line.contains("line 2")));
    assert!(!rendered.iter().any(|line| line.contains("ctrl+o")));
}

#[test]
fn generic_tool_output_is_hidden_at_every_verbosity_without_mutating_history() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    let body = (0..50)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new("tool", format!("mcp_custom_tool: completed\n{body}"))
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                success: true,
                ..Default::default()
            }),
    );
    let history = state.history.clone();

    state.verbosity = Verbosity::Low;
    let low = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    state.verbosity = Verbosity::High;
    let high = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(!low.iter().any(|line| line.contains("line 25")));
    assert!(!low.iter().any(|line| line.contains("line 49")));
    assert!(!high.iter().any(|line| line.contains("line 49")));
    assert!(!high.iter().any(|line| line.contains("… +31 lines")));
    assert!(!high.iter().any(|line| line.contains("line 25")));
    assert!(!low.iter().any(|line| line.contains("(ctrl+o all")));
    assert!(!high.iter().any(|line| line.contains("(ctrl+o all")));
    assert!(state.history == history);
}

#[test]
fn low_verbosity_generic_output_stays_hidden_when_expanded() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    let long_line = "result line with enough words to exceed a narrow terminal width";
    state.history.push(
        ChatMessage::new("tool", format!("mcp_custom_tool: {long_line}"))
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    // Generic result bodies stay hidden even if an old expansion index is
    // present in the view state.
    let collapsed = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(collapsed.len(), 2);
    assert_eq!(collapsed[0], "• Ran");
    assert!(
        collapsed[1].contains("McpCustomTool") && !collapsed[1].contains("(ctrl+o all"),
        "generic action remains without an expansion hint: {collapsed:?}"
    );

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1], 30, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(!expanded.iter().any(|line| line.contains("result line")));
}

#[test]
fn collapsed_tool_bodies_cap_wrapped_unicode_output_at_five_rows() {
    use rustcode::controller::Verbosity;
    use unicode_width::UnicodeWidthStr;

    let body = (0..8)
        .map(|index| Line::from(format!("{index}: {}", "日本語の出力".repeat(5))))
        .collect::<Vec<_>>();
    let width = 24;

    let generic =
        super::indent_generic_tool_body(body.clone(), &Verbosity::Low, width, false, false);
    let command =
        super::indent_tool_result_body(body, "run_command", &Verbosity::Low, width, false);

    for (kind, rendered) in [("generic", generic), ("command", command)] {
        assert!(
            rendered.len() <= 5,
            "collapsed {kind} output must occupy at most five visual rows: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .all(|line| line.to_string().width() <= width as usize),
            "collapsed {kind} output must wrap to the terminal width: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|line| line.to_string().contains('日')),
            "Unicode output remains intact: {rendered:?}"
        );
    }
}

#[test]
fn committed_shell_output_is_five_rows_when_collapsed_and_complete_when_expanded() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"spotify-cli p --help"}"#.to_owned(),
        }]),
    );
    let body = (0..12)
        .map(|index| format!("status {index}: {}", "日本語の出力".repeat(5)))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 0\nstdout:\n{body}"),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        }),
    );

    let collapsed = super::render_committed_tool_result_group(&state, &[1], 36, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(!collapsed[0].contains("ctrl+o"), "{collapsed:?}");
    assert!(
        collapsed.last().unwrap().contains("(ctrl+o all"),
        "{collapsed:?}"
    );
    let body_start = collapsed
        .iter()
        .position(|line| line.contains("│"))
        .expect("rendered shell output begins below its command header");
    assert!(
        collapsed.len() - body_start - 1 <= 5,
        "collapsed body rows: {collapsed:?}"
    );
    assert!(collapsed.iter().any(|line| line.contains("lines")));
    assert!(collapsed.iter().any(|line| line.contains('日')));

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1], 36, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    for index in 0..12 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("status {index}:"))),
            "expanded output preserves status {index}: {expanded:?}"
        );
    }
    assert!(!expanded.iter().any(|line| line.contains("(ctrl+o all")));
}

#[test]
fn command_preview_wrap_reuse_preserves_styled_lines_and_hint_state() {
    use super::tool_transcript::{
        COLLAPSED_TOOL_BODY_MAX_LINES, command_preview_body, indent_tool_result_body,
    };
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use rustcode::controller::Verbosity;

    for width in [18, 24, 80] {
        for body in [
            vec![
                Line::from(vec![
                    Span::styled("status: ", Style::default().fg(Color::Green)),
                    Span::styled("✓ готово 日", Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(Span::styled(
                    "short row",
                    Style::default().fg(Color::Yellow),
                )),
            ],
            (0..12)
                .map(|index| {
                    Line::from(vec![
                        Span::styled(format!("{index}: "), Style::default().fg(Color::Cyan)),
                        Span::styled(
                            "日本語の出力 with styled tail",
                            Style::default()
                                .fg(Color::Magenta)
                                .add_modifier(Modifier::ITALIC),
                        ),
                    ])
                })
                .collect(),
        ] {
            for expanded in [false, true] {
                let full = indent_tool_result_body(
                    body.clone(),
                    "run_command",
                    &Verbosity::Low,
                    width,
                    true,
                );
                let expected_hint = !expanded && full.len() > COLLAPSED_TOOL_BODY_MAX_LINES;
                let expected_body = indent_tool_result_body(
                    body.clone(),
                    "run_command",
                    &Verbosity::Low,
                    width,
                    expanded,
                );
                let (actual_body, actual_hint) = command_preview_body(
                    body.clone(),
                    "run_command",
                    &Verbosity::Low,
                    width,
                    expanded,
                );

                assert_eq!(
                    actual_body, expected_body,
                    "width={width}, expanded={expanded}"
                );
                assert_eq!(
                    actual_hint, expected_hint,
                    "width={width}, expanded={expanded}"
                );
            }
        }
    }
}

#[test]
fn low_verbosity_keeps_errors_and_exit_status_visible() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo check --tests"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "run_command: exit code: 1\nstdout:\ncompiling\nstderr:\nerror: build failed",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: false,
            exit_code: Some(1),
            ..Default::default()
        }),
    );

    // The shell invocation and exit status stay visible while output is
    // collapsed; expanding reveals stderr without protocol noise (#1568).
    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered.iter().any(|line| line.contains("exit 1")),
        "exit status stays: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("(ctrl+o all")),
        "fully visible shell output needs no expansion hint: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("build failed")),
        "short error output remains visible while collapsed: {rendered:?}"
    );

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        expanded.iter().any(|line| line.contains("build failed")),
        "expanded shell result reveals stderr: {expanded:?}"
    );
    assert!(
        !expanded.iter().any(|line| line.contains("stdout:")),
        "protocol labels are stripped: {rendered:?}"
    );
}

#[test]
fn low_verbosity_long_generic_body_stays_hidden_when_expanded() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "mcp_custom_tool".to_owned(),
            arguments: r#"{"path":"src"}"#.to_owned(),
        }]),
    );
    let body = (0..50)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new("tool", format!("mcp_custom_tool: completed\n{body}"))
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "mcp_custom_tool".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    state.expanded_thoughts.insert(1);
    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(!rendered.iter().any(|line| line.contains("line 0")));
    assert!(!rendered.iter().any(|line| line.contains("line 49")));
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("+") && line.contains("lines")),
        "no omission marker when expanded: {rendered:?}"
    );
}

#[test]
fn expanded_command_body_renders_full_not_window() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"make test"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
        ]));
    let body = (0..30)
        .map(|i| format!("output line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 0\nstdout:\n{body}"),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        }),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    // Collapsed: the five-row preview keeps the output head and tail.
    let collapsed = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("Bash") && line.contains("(ctrl+o all")),
        "collapsed command carries hint: {collapsed:?}"
    );
    assert!(
        !collapsed.iter().any(|line| line.contains("output line 15")),
        "collapsed command hides body: {collapsed:?}"
    );

    // Expanded: full body, no omission marker.
    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    for i in 0..30 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("output line {i}"))),
            "line {i} present when expanded: {expanded:?}"
        );
    }
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("+") && line.contains("lines")),
        "no omission marker when expanded: {expanded:?}"
    );
}

#[test]
fn homogeneous_command_batch_collapses_with_hint_and_expands_fully() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"make test"}"#.to_owned(),
        }]),
    );
    let body = (0..20)
        .map(|i| format!("homo line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 0\nstdout:\n{body}"),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            success: true,
            exit_code: Some(0),
            ..Default::default()
        }),
    );

    // Homogeneous command output stays compact until the user expands it.
    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(rendered.iter().any(|line| line.contains("(ctrl+o all")));
    assert!(rendered.iter().any(|line| line.contains("homo line 0")));
    assert!(rendered.iter().any(|line| line.contains("homo line 19")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("+") && line.contains("lines")),
        "collapsed command output has an omission marker: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("ctrl+o")),
        "collapsed command output has an expand hint: {rendered:?}"
    );

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    for i in 0..20 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("homo line {i}"))),
            "expanded line {i} is present: {expanded:?}"
        );
    }
}

#[test]
fn low_verbosity_frame_keeps_hierarchy_and_diff_visible() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/new.rs","content":"pub fn new() {}"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "write_to_file: wrote 'src/new.rs' (1 lines, 15 bytes)",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "write_to_file".to_owned(),
            success: true,
            changed_paths: vec!["src/new.rs".to_owned()],
            ..Default::default()
        }),
    );

    // Inspect an actual rendered terminal frame (not just the group block):
    // hierarchy (Wrote heading + path child), diff content, and no high-
    // verbosity JSON/protocol noise (#1568).
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("Wrote"), "{rendered}");
    assert!(rendered.contains("src/new.rs"), "{rendered}");
    assert!(rendered.contains("pub fn new"), "{rendered}");
}

#[test]
fn low_verbosity_frame_shows_only_bash_output_and_edit_diffs() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    let calls = [
        (
            "call-read",
            "view_file",
            r#"{"TargetFile":"src/read.rs"}"#,
            "view_file: read-payload-marker",
        ),
        (
            "call-mcp",
            "mcp_custom_tool",
            r#"{"query":"lookup"}"#,
            "mcp_custom_tool: mcp-payload-marker",
        ),
        (
            "call-skill",
            "use_skill",
            r#"{"name":"test-skill"}"#,
            "use_skill: skill-payload-marker",
        ),
        (
            "call-bash",
            "run_command",
            r#"{"command":"printf bash-output-marker"}"#,
            "run_command: exit code: 0\nbash-output-marker",
        ),
        (
            "call-edit",
            "write_to_file",
            r#"{"path":"src/added.rs","content":"edit-diff-marker"}"#,
            "write_to_file: wrote 'src/added.rs' (1 lines, 15 bytes)",
        ),
    ];
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(
            calls
                .iter()
                .map(|(id, name, arguments, _)| ToolCallRef {
                    id: (*id).to_owned(),
                    name: (*name).to_owned(),
                    arguments: (*arguments).to_owned(),
                })
                .collect(),
        ),
    );
    for (id, name, _, result) in &calls {
        let mut record = ToolResultRecord {
            tool_name: (*name).to_owned(),
            success: true,
            ..Default::default()
        };
        if *name == "write_to_file" {
            record.changed_paths = vec!["src/added.rs".to_owned()];
        }
        if *name == "mcp_custom_tool" {
            record.success = false;
        }
        state.history.push(
            ChatMessage::new("tool", *result)
                .answering(Some((*id).to_owned()))
                .with_tool_result(record),
        );
    }

    let assert_no_hidden_payloads = |rendered: &str| {
        assert!(!rendered.contains("read-payload-marker"), "{rendered}");
        assert!(!rendered.contains("mcp-payload-marker"), "{rendered}");
        assert!(!rendered.contains("skill-payload-marker"), "{rendered}");
        assert!(!rendered.contains("specific_error_marker"), "{rendered}");
        assert!(
            rendered.contains("failed"),
            "failure status stays visible: {rendered}"
        );
    };

    state.verbosity = Verbosity::High;
    let high = render_state_to_text(&mut state, 100, 40);
    assert_no_hidden_payloads(&high);
    // The command invocation remains visible at high verbosity.
    assert!(!high.contains("edit-diff-marker"), "{high}");

    state.verbosity = Verbosity::Low;
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 100);
    assert_eq!(candidates, [4, 5], "only Bash and edit bodies expand");

    let collapsed = render_state_to_text(&mut state, 100, 40);
    assert_no_hidden_payloads(&collapsed);
    assert!(collapsed.contains("bash-output-marker"), "{collapsed}");
    assert!(collapsed.contains("edit-diff-marker"), "{collapsed}");

    state.expanded_thoughts.extend(candidates);
    let expanded = render_state_to_text(&mut state, 100, 40);
    assert_no_hidden_payloads(&expanded);
    assert!(expanded.contains("bash-output-marker"), "{expanded}");
    assert!(expanded.contains("edit-diff-marker"), "{expanded}");
}

#[test]
fn default_verbosity_is_high() {
    assert_eq!(
        rustcode::controller::Verbosity::default(),
        rustcode::controller::Verbosity::High
    );
}

#[test]
fn completed_edits_have_a_distinct_transcript_heading() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "replace_file_content".to_owned(),
            arguments: r#"{"path":"src/main.rs"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "replace_file_content: successfully edited")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "replace_file_content".to_owned(),
                arguments_hash: String::new(),
                success: true,
                changed_paths: vec!["src/main.rs".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Edited");
    assert_eq!(rendered[1], "  └ src/main.rs");
}

#[test]
fn committed_file_write_is_labeled_as_a_write() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/new.rs","content":"pub fn new() {}"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "write_to_file: wrote src/new.rs")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "write_to_file".to_owned(),
                success: true,
                changed_paths: vec!["src/new.rs".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Wrote", "  └ src/new.rs"]);
}

#[test]
fn low_verbosity_write_shows_added_lines_preview() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/new.rs","content":"pub fn new() {}\npub fn old() {}"}"#
                .to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "write_to_file: wrote 'src/new.rs' (2 lines, 30 bytes)",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "write_to_file".to_owned(),
            success: true,
            changed_paths: vec!["src/new.rs".to_owned()],
            ..Default::default()
        }),
    );

    // Low verbosity keeps the path label but also shows the added lines
    // instead of only the filename/count summary (#1567).
    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(rendered[0], "• Wrote");
    assert!(
        rendered.iter().any(|line| line.contains("src/new.rs")),
        "path label stays: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("pub fn new")),
        "added content renders inline: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("ctrl+o")),
        "fully visible edit preview needs no expansion hint: {rendered:?}"
    );
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(candidates, [1]);
}

#[test]
fn high_verbosity_keeps_actual_file_diff_visible() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/main.rs","content":"let value = 2;"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "write_to_file: wrote 'src/main.rs'")
            .answering(Some("call-1".to_owned()))
            .with_diff(Some(
                "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -20 +20 @@\n-let value = 1;\n+let value = 2;\n".to_owned(),
            ))
            .with_tool_result(ToolResultRecord {
                tool_name: "write_to_file".to_owned(),
                success: true,
                changed_paths: vec!["src/main.rs".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("src/main.rs (+1 -1)")),
        "{rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("20 -let value = 1;")),
        "{rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("20 +let value = 2;")),
        "{rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("@@") || line.contains("wrote")),
        "{rendered:?}"
    );
}

#[test]
fn low_verbosity_write_expand_round_trip_changes_body_and_hint() {
    use rustcode::controller::{
        ChatMessage, ExpandOutcome, ToolCallRef, ToolResultRecord, Verbosity,
    };

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    let content = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\\n");
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: format!(r#"{{"path":"src/big.rs","content":"{content}"}}"#),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "write_to_file: wrote 'src/big.rs' (20 lines, 100 bytes)",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "write_to_file".to_owned(),
            success: true,
            changed_paths: vec!["src/big.rs".to_owned()],
            ..Default::default()
        }),
    );

    let render = |state: &RenderState| {
        super::render_committed_tool_result_group(state, &[1], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
    };
    let collapsed = render(&state);
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("+") && line.contains("lines")),
        "collapsed preview truncates with an omitted count: {collapsed:?}"
    );
    assert!(
        collapsed.iter().any(|line| line.contains("ctrl+o")),
        "collapsed preview advertises expansion: {collapsed:?}"
    );

    let mut focus = None;
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    let (outcome, _) = rustcode::controller::toggle_expanded_bodies(
        &mut state.expanded_thoughts,
        &mut focus,
        &candidates,
    );
    assert_eq!(outcome, ExpandOutcome::Expanded(1));
    let expanded = render(&state);
    assert!(
        expanded.iter().any(|line| line.contains("line 19")),
        "expanded body reveals the tail: {expanded:?}"
    );
    assert!(
        !expanded.iter().any(|line| line.contains("ctrl+o")),
        "expanded body drops the hint: {expanded:?}"
    );
    assert!(
        expanded.len() > collapsed.len(),
        "expand visibly changes the body: {collapsed:?} -> {expanded:?}"
    );
}

#[test]
fn write_noop_and_failure_keep_truthful_status_without_hint() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    for (name, tool, result, success) in [
        (
            "noop",
            "replace_file_content",
            "replace_file_content: already applied; no changes made to 'src/main.rs'",
            true,
        ),
        (
            "failed",
            "write_to_file",
            "write_to_file: error: cannot write 'src/new.rs': permission denied",
            false,
        ),
    ] {
        let mut state = RenderState::new();
        state.verbosity = Verbosity::Low;
        state.history.push(
            ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
                id: "call-1".to_owned(),
                name: tool.to_owned(),
                arguments: r#"{"path":"src/main.rs","content":"hi"}"#.to_owned(),
            }]),
        );
        state.history.push(
            ChatMessage::new("tool", result)
                .answering(Some("call-1".to_owned()))
                .with_tool_result(ToolResultRecord {
                    tool_name: tool.to_owned(),
                    success,
                    ..Default::default()
                }),
        );
        let rendered = super::render_committed_tool_result_group(&state, &[1], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        assert!(
            !rendered.iter().any(|line| line.contains("ctrl+o")),
            "{name} carries no expand hint: {rendered:?}"
        );
        let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
        assert!(
            candidates.is_empty(),
            "{name} is not an expand candidate: {candidates:?}"
        );
    }
}

#[test]
fn low_verbosity_edit_diff_wraps_at_narrow_width() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/new.rs","content":"pub fn a_very_long_function_name_that_exceeds_width() {}"}"#
                .to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "write_to_file: wrote 'src/new.rs' (1 lines, 50 bytes)",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "write_to_file".to_owned(),
            success: true,
            changed_paths: vec!["src/new.rs".to_owned()],
            ..Default::default()
        }),
    );

    let width = 24u16;
    let rendered = super::render_committed_tool_result_group(&state, &[1], width, false);
    for line in &rendered {
        let w = line.to_string().chars().count();
        assert!(
            w <= width as usize + super::EXPAND_HINT_WIDTH as usize,
            "narrow preview stays within width {width}: {line:?}"
        );
    }
    let joined = rendered
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("src/new.rs"),
        "narrow preview keeps path: {joined:?}"
    );
    assert!(
        joined.contains("pub") && joined.contains("fn"),
        "narrow preview keeps content across wraps: {joined:?}"
    );
}

#[test]
fn committed_batched_edits_with_casing_aliases_group_under_edited() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "replace_file_content".to_owned(),
                arguments: r#"{"TargetFile":"src/game/engine.ts"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "WriteFile".to_owned(),
                arguments: r#"{"path":"src/App.tsx"}"#.to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "replace_file_content: ok")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "replace_file_content".to_owned(),
                success: true,
                changed_paths: vec!["src/game/engine.ts".to_owned()],
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "WriteFile: ok")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "WriteFile".to_owned(),
                success: true,
                changed_paths: vec!["src/App.tsx".to_owned()],
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Edited");
    assert_eq!(rendered[1], "  └ src/game/engine.ts");
    assert_eq!(rendered[2], "    src/App.tsx");
}

#[test]
fn exploration_results_group_and_deduplicate_child_rows() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-list-1".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-search".to_owned(),
                name: "grep".to_owned(),
                arguments: r#"{"pattern":"renderer","path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-list-2".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
        ]));
    for (id, name, content) in [
        ("call-list-1", "list_directory", "list_directory: ui/"),
        ("call-search", "grep", "grep: src/ui/mod.rs:1"),
        ("call-list-2", "list_directory", "list_directory: ui/"),
    ] {
        state.history.push(
            ChatMessage::new("tool", content)
                .answering(Some(id.to_owned()))
                .with_tool_result(ToolResultRecord {
                    tool_name: name.to_owned(),
                    arguments_hash: String::new(),
                    success: true,
                    exit_code: None,
                    changed_paths: Vec::new(),
                    truncated: false,
                    full_output_artifact: None,
                    ..Default::default()
                }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2, 3], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Explored");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| *line == "  └ List src")
            .count(),
        1
    );
    assert!(
        rendered
            .iter()
            .any(|line| line == "    Search renderer in src")
    );
}

#[test]
fn exploration_results_match_repeated_calls_without_ids_in_order() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord};

    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "unused-1".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"src"}"#.to_owned(),
            },
            ToolCallRef {
                id: "unused-2".to_owned(),
                name: "list_directory".to_owned(),
                arguments: r#"{"path":"tests"}"#.to_owned(),
            },
        ]));
    for content in ["list_directory: ui/", "list_directory: fixtures/"] {
        state.history.push(
            ChatMessage::new("tool", content).with_tool_result(ToolResultRecord {
                tool_name: "list_directory".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
        );
    }

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line == "  └ List src"));
    assert!(rendered.iter().any(|line| line == "    List tests"));
}

#[test]
fn command_preview_preserves_the_output_tail() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"cargo test"}"#.to_owned(),
        }]),
    );
    let body = (0..20)
        .map(|index| format!("line {index}"))
        .chain(std::iter::once("error: build failed".to_owned()))
        .collect::<Vec<_>>()
        .join("\n");
    state.history.push(
        ChatMessage::new(
            "tool",
            format!("run_command: exit code: 1\nstderr:\n{body}"),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "run_command".to_owned(),
            arguments_hash: String::new(),
            success: false,
            exit_code: Some(1),
            changed_paths: Vec::new(),
            truncated: false,
            full_output_artifact: None,
            ..Default::default()
        }),
    );

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    // The collapsed window keeps the beginning and actionable error tail.
    assert!(
        rendered.iter().any(|line| line.contains("… +")),
        "collapsed output reports omitted rows: {rendered:?}"
    );
    assert!(rendered.iter().any(|line| line.contains("line 0")));
    assert!(rendered.iter().any(|line| line.contains("line 19")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("error: build failed"))
    );

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    for index in 0..20 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("line {index}"))),
            "expanded output keeps line {index}: {expanded:?}"
        );
    }
}

#[test]
fn expanded_generic_tool_preserves_only_its_action_row() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "custom_lookup".to_owned(),
            arguments: r#"{"query":"renderer"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "custom_lookup: first result\nsecond result")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "custom_lookup".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: None,
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );
    state.expanded_thoughts.insert(1);

    let rendered = super::render_committed_history_block(&state, 1, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("CustomLookup query=\"renderer\"")),
        "the compact generic action row remains: {rendered:?}"
    );
    assert!(!rendered.iter().any(|line| line.contains("first result")));
    assert!(!rendered.iter().any(|line| line.contains("second result")));
}

#[test]
fn mixed_batch_command_entry_shows_expand_hint_and_body() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\nstdout:\nM src/main.rs")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Bash") && line.contains("(ctrl+o all")),
        "fully visible command needs no expansion hint: {rendered:?}"
    );

    // The hint belongs to the row it describes. Appending it to the last
    // wrapped line of the command preview lands it between the two tool rows,
    // so it reads as annotating the `GetTime` row below and splits the group
    // (#1541).
    assert_eq!(
        rendered,
        [
            "• Ran",
            "  └ Bash git status --short",
            "  └   │ M src/main.rs",
            "    GetTime",
        ],
        "each hint stays on the row of the entry it expands: {rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.trim_start().starts_with("(ctrl+o")),
        "the hint must never occupy a row of its own: {rendered:?}"
    );

    state.expanded_thoughts.insert(1);
    let expanded = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        expanded.iter().any(|line| line.contains("M src/main.rs")),
        "expanded command should reveal its body: {expanded:?}"
    );
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("Bash") && line.contains("(ctrl+o all")),
        "an already expanded row must not advertise the expand hint: {expanded:?}"
    );
}

#[test]
fn ctrl_o_round_trips_the_last_collapsed_tool_body() {
    use rustcode::controller::{
        ChatMessage, ExpandOutcome, ToolCallRef, ToolResultRecord, Verbosity,
    };

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\nstdout:\nM src/main.rs")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    let render = |state: &RenderState| {
        super::render_committed_tool_result_group(state, &[1, 2], 80, false)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
    };
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(candidates, [1], "only the command body is collapsible");
    assert!(
        render(&state)
            .iter()
            .any(|line| line.contains("M src/main.rs")),
        "the command body starts collapsed"
    );

    // The press runs through the seam against the expand state this view
    // already carries: the engine owns the transition, the render layer only
    // holds the set and the focus it reports back (#1431).
    let mut focus = None;
    let (outcome, notice) = rustcode::controller::toggle_expanded_bodies(
        &mut state.expanded_thoughts,
        &mut focus,
        &candidates,
    );
    assert_eq!(outcome, ExpandOutcome::Expanded(1));
    assert_eq!(notice, "Expanded tool output");
    assert_eq!(focus, Some(1));
    let expanded = render(&state);
    assert!(
        expanded.iter().any(|line| line.contains("M src/main.rs")),
        "the expanded command body renders inline: {expanded:?}"
    );
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("Bash") && line.contains("(ctrl+o all")),
        "an expanded row drops the hint it carried while collapsed: {expanded:?}"
    );

    // Expansion survives new output and scrolling: the expanded set lives in
    // session state, not in the committed scrollback, so neither can reset it.
    state.history.push(ChatMessage::new("user", "and now?"));
    let mut transcript = TranscriptState::default();
    transcript.scroll_up(5);
    assert_eq!(transcript.scroll_rows(), 5);
    assert!(
        render(&state)
            .iter()
            .any(|line| line.contains("M src/main.rs")),
        "expansion survives new output and scrolling"
    );

    let (outcome, notice) = rustcode::controller::toggle_expanded_bodies(
        &mut state.expanded_thoughts,
        &mut focus,
        &candidates,
    );
    assert_eq!(
        outcome,
        ExpandOutcome::Collapsed(1),
        "a second press collapses what the first expanded"
    );
    assert_eq!(notice, "Collapsed tool output");
    assert!(state.expanded_thoughts.is_empty());
    assert_eq!(focus, None);
    assert!(
        render(&state)
            .iter()
            .any(|line| line.contains("M src/main.rs")),
        "collapsing retains the bounded visible preview: {:?}",
        render(&state)
    );
}

#[test]
fn homogeneous_command_batch_has_independent_collapsible_candidates() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"cargo check --tests"}"#.to_owned(),
            },
        ]));
    for (id, output) in [
        (
            "call-1",
            "run_command: exit code: 0\nstdout:\nM src/main.rs",
        ),
        ("call-2", "run_command: exit code: 0\nstdout:\nok"),
    ] {
        state.history.push(
            ChatMessage::new("tool", output)
                .answering(Some(id.to_owned()))
                .with_tool_result(ToolResultRecord {
                    tool_name: "run_command".to_owned(),
                    success: true,
                    exit_code: Some(0),
                    ..Default::default()
                }),
        );
    }

    // Homogeneous command-only batches expose each result as an independent
    // expand candidate, preserving single-entry targeting.
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert!(
        candidates == [1, 2],
        "each command result is independently collapsible: {candidates:?}"
    );

    // Both commands still render stably, one summary each, with an action hint.
    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("Ran $"))
            .count(),
        2,
        "each homogeneous command keeps its own summary: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|line| line.contains("(ctrl+o all")),
        "fully visible commands need no expansion hints: {rendered:?}"
    );
}

#[test]
fn later_command_only_group_is_independently_expandable_after_edit() {
    use rustcode::controller::{
        ChatMessage, ExpandOutcome, ToolCallRef, ToolResultRecord, Verbosity,
    };

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "write_to_file".to_owned(),
            arguments: r#"{"path":"src/earlier.rs","content":"fn earlier() {\n}"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            "write_to_file: wrote 'src/earlier.rs' (1 lines, 14 bytes)",
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "write_to_file".to_owned(),
            success: true,
            changed_paths: vec!["src/earlier.rs".to_owned()],
            ..Default::default()
        }),
    );
    state.history.push(ChatMessage::new("user", "and then?"));
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-2".to_owned(),
            name: "run_command".to_owned(),
            arguments: r#"{"command":"git status --short"}"#.to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\nstdout:\nM src/main.rs")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );

    // Both edit and command bodies are candidates, and the existing no-focus
    // rule targets the newest one. The earlier edit remains independently
    // addressable.
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(candidates, [1, 4], "candidate ordering is stable");

    let mut focus = None;
    let (outcome, _) = rustcode::controller::toggle_expanded_bodies(
        &mut state.expanded_thoughts,
        &mut focus,
        &candidates,
    );
    assert_eq!(outcome, ExpandOutcome::Expanded(4));
    let rendered = super::render_committed_tool_result_group(&state, &[4], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        rendered.iter().any(|line| line.contains("M src/main.rs")),
        "expanding reveals the later command body: {rendered:?}"
    );
    assert!(state.expanded_thoughts.contains(&4));
    assert!(!state.expanded_thoughts.contains(&1));
}

#[test]
fn mixed_batch_keeps_command_collapsible_alongside_generic() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    state
        .history
        .push(ChatMessage::new("assistant", "").with_tool_calls(vec![
            ToolCallRef {
                id: "call-1".to_owned(),
                name: "run_command".to_owned(),
                arguments: r#"{"command":"git status --short"}"#.to_owned(),
            },
            ToolCallRef {
                id: "call-2".to_owned(),
                name: "get_time".to_owned(),
                arguments: "{}".to_owned(),
            },
        ]));
    state.history.push(
        ChatMessage::new("tool", "run_command: exit code: 0\nstdout:\nM src/main.rs")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                success: true,
                exit_code: Some(0),
                ..Default::default()
            }),
    );
    state.history.push(
        ChatMessage::new("tool", "get_time: Thursday, 08:30")
            .answering(Some("call-2".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "get_time".to_owned(),
                success: true,
                ..Default::default()
            }),
    );

    // The command remains expandable in a mixed batch; hidden generic output
    // offers no expansion candidate.
    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(candidates, [1]);
    let rendered = super::render_committed_tool_result_group(&state, &[1, 2], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("Bash") && line.contains("ctrl+o")),
        "fully visible mixed command needs no hint: {rendered:?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("GetTime") && line.contains("ctrl+o")),
        "fully visible mixed generic output needs no hint: {rendered:?}"
    );
}

#[test]
fn ctrl_o_without_a_collapsed_body_reports_it_instead_of_doing_nothing() {
    use rustcode::controller::ChatMessage;
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    let mut expanded = std::collections::HashSet::new();
    let mut focus = None;

    let (outcome, notice) =
        rustcode::controller::toggle_expanded_bodies(&mut expanded, &mut focus, &[]);

    assert_eq!(
        outcome,
        rustcode::controller::ExpandOutcome::NothingToExpand
    );
    assert_eq!(
        notice, "Nothing to expand",
        "an empty press must still say something"
    );
    assert!(expanded.is_empty());

    // The notice is the press's only visible effect, so it has to reach the
    // frame: the frontend installs it on the view the next render reads.
    state.transient_notice = Some(notice.to_owned());
    let rendered = render_state_to_text(&mut state, 80, 24);
    assert!(rendered.contains("Nothing to expand"), "{rendered}");
}

#[test]
fn collapses_image_markers_to_chips() {
    // Plain text is untouched.
    assert_eq!(collapse_image_markers("hello world"), "hello world");

    // A single marker becomes a numbered chip, surrounding text preserved.
    assert_eq!(
        collapse_image_markers("look ![image](file:///tmp/a.png) here"),
        "look [Image #1] here"
    );

    // Multiple markers increment.
    assert_eq!(
        collapse_image_markers("![image](file:///tmp/a.png)![image](file:///tmp/b.png)"),
        "[Image #1][Image #2]"
    );

    // Unclosed marker (mid-paste) is left as-is from the marker onward.
    let unclosed = "text ![image](file:///tmp/a";
    assert_eq!(collapse_image_markers(unclosed), unclosed);
}

#[test]
fn pasted_image_and_text_chips_use_accent_text() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new(
        "user",
        "see ![image](file:///tmp/a.png) and <!--PASTE:12:pasted text-->",
    ));

    let block = super::render_committed_history_block(&state, 0, 100);
    let marker_text = block
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.style.fg == Some(super::COLOR_PRIMARY()))
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(marker_text.contains("[Image #1]"));
    assert!(marker_text.contains("[Pasted Text #1 (12 chars)]"));
    assert!(
        block
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| {
                span.style.fg == Some(super::COLOR_PRIMARY())
                    && span.style.add_modifier.contains(Modifier::BOLD)
            })
            .count()
            > 1
    );
}

#[test]
fn code_blocks_render_as_lightweight_transcript_rows() {
    use super::{AssistantRenderOptions, render_assistant_message};
    let content = "```text\nWhy Rust Outshines C#\n\nA short line\n```";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    let width: u16 = 80;
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: width,
            show_picker: false,
            last_copy_text: None,
        },
    );

    // The body remains copyable without a language/copy header or full-width panel.
    assert_eq!(copies.len(), 1);
    let rendered = lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Why Rust Outshines C#"));
    assert!(rendered.contains("A short line"));
    assert!(!rendered.contains("Copy 📋"));
    assert!(lines.iter().all(|line| line.width() < width as usize));
}

#[test]
fn streamed_markdown_fences_keep_adversarial_content_in_the_code_cell() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let streaming = concat!(
        "Before\n\n",
        "````rust\n",
        "let marker = \"```\";\n",
        "```\n",
        "let still_code = true;\n"
    );
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        streaming,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: true,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );
    let streaming_text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(streaming_text.contains("let marker = \"```\";"));
    assert!(streaming_text.contains("let still_code = true;"));

    let completed = concat!(
        "Before\n\n",
        "~~~text\nfirst\n~~~\n\n",
        "```rust\nsecond\n```\n\n",
        "After"
    );
    lines.clear();
    copies.clear();
    render_assistant_message(
        completed,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );
    let completed_text: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(completed_text.contains("first"));
    assert!(completed_text.contains("second"));
    assert!(completed_text.contains("After"));
    assert_eq!(copies.len(), 2, "both completed fences need copy targets");
}

#[test]
fn diff_code_blocks_preserve_patch_context_like_codex() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let content = "```diff\n--- a/src/temp.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-old\n-removed\n```";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(rendered.contains("a/src/temp.rs"));
    assert!(rendered.contains("/dev/null"));
    assert!(rendered.contains("@@ -1,2"));
    assert!(rendered.contains("removed"));
}

#[test]
fn thinking_with_tool_calls_hides_serialized_tool_blocks() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let content = concat!(
        "<think>Planning the next command.</think>\n\n",
        "```tool\n",
        r#"{"name":"run_command","arguments":{"command":"git status"}}"#,
        "\n```"
    );
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(rendered.contains("Thought"));
    assert!(rendered.contains("Planning the next command."));
    assert!(!rendered.contains("run_command"));
    assert!(!rendered.contains("git status"));
    assert!(!rendered.contains("Build"));
}

#[test]
fn thought_parser_collapses_multiple_blocks() {
    let (answer, preview) = split_thought_blocks(
        "<think>First useful thought\nmore detail</think>answer\n<think>Second thought</think>",
    );
    assert_eq!(answer, "answer");
    assert_eq!(preview.as_deref(), Some("First useful thought"));
}

#[test]
fn thought_parser_drops_unclosed_block_from_answer() {
    let (answer, preview) = split_thought_blocks("before\n<think>Planning the next action");
    assert_eq!(answer, "before");
    assert_eq!(preview.as_deref(), Some("Planning the next action"));
}

#[test]
fn thought_parser_handles_missing_open_tag() {
    let (answer, preview) =
        split_thought_blocks("Reasoning about user request.\n</think>\n\nFinal response");
    assert_eq!(answer, "Final response");
    assert_eq!(preview.as_deref(), Some("Reasoning about user request."));
}

#[test]
fn thought_parser_captures_preamble_before_think_tag() {
    let raw = "Okay, the user is asking hello how are you, which I should respond to politely.\n\nFirst, I must check skills.\n\n<think>\nI will provide a standard friendly response.\n</think>\n\nHello! I am doing well, thank you for asking.";
    let (answer, preview) = split_thought_blocks(raw);
    assert_eq!(answer, "Hello! I am doing well, thank you for asking.");
    assert_eq!(
        preview.as_deref(),
        Some("Okay, the user is asking hello how are you, which I should respond to politely.")
    );
}

#[test]
fn thought_preview_keeps_short_text_unchanged() {
    assert_eq!(
        truncate_thought_preview("Analyzing Paste Events", 24),
        "Analyzing Paste Events"
    );
}

#[test]
fn thought_preview_truncates_to_one_display_line() {
    assert_eq!(
        truncate_thought_preview(
            "The user has made a request with contradictory instructions.",
            24
        ),
        "The user has made a req…"
    );
}

#[test]
fn thought_preview_does_not_split_wide_or_multibyte_characters() {
    let result = truncate_thought_preview("分析しています 🚀", 10);
    assert!(result.width() <= 10);
    assert!(result.is_char_boundary(result.len()));
}

#[test]
fn test_thinking_renders_metadata_and_summary() {
    use super::{AssistantRenderOptions, render_assistant_message};
    use rustcode::controller::TokenUsage;

    let content =
        "<think>\nUnderstanding the history issue.\nTracing line by line.\n</think>\nDone";
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        content,
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: Some(TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 400,
                total_tokens: 1400,
                cached_tokens: None,
                ..Default::default()
            }),
            response_time_ms: Some(3000),
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    assert_eq!(lines[0].spans[1].content, "Thought for 3s, 1.4k tokens");
    assert_eq!(lines[0].spans[0].content, "▸ ");
    assert_eq!(
        lines[1].spans[0].content,
        "  Understanding the history issue."
    );
    let rendered: String = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
    assert!(!rendered.contains("Tracing line by line."));
}

#[test]
fn thinking_metadata_uses_thought_stats_not_full_response_stats() {
    use super::{AssistantRenderOptions, render_assistant_message};
    use rustcode::controller::TokenUsage;

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "<think>Planning the answer.</think>Final answer.",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: Some(TokenUsage {
                prompt_tokens: 1000,
                completion_tokens: 900,
                total_tokens: 1900,
                cached_tokens: None,
                ..Default::default()
            }),
            response_time_ms: Some(9000),
            thought_time_ms: Some(1250),
            thought_tokens: Some(42),
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    assert_eq!(lines[0].spans[1].content, "Thought for 1.2s, 42 tokens");
}

#[test]
fn test_tool_result_follows_skips_hidden_notices() {
    use super::tool_result_follows;
    use rustcode::controller::ChatMessage;

    let history = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("system", "[harness: stopped after 13 tool round(s)]"),
        ChatMessage::new("tool", "tool output"),
    ];
    assert!(tool_result_follows(&history, 0));

    let history_direct = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("tool", "tool output"),
    ];
    assert!(tool_result_follows(&history_direct, 0));

    let history_no_tool = vec![
        ChatMessage::new("assistant", "calling tool"),
        ChatMessage::new("user", "hello"),
    ];
    assert!(!tool_result_follows(&history_no_tool, 0));
}

#[test]
fn tool_result_spacing_targets_next_assistant() {
    use super::tool_result_needs_assistant_gap;
    use rustcode::controller::ChatMessage;

    let direct_assistant = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("assistant", "<think>planning</think>answer"),
    ];
    assert!(tool_result_needs_assistant_gap(&direct_assistant, 0));

    let hidden_notice_then_assistant = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("system", "[harness: stopped after 1 tool round(s)]"),
        ChatMessage::new("assistant", "<think>planning</think>answer"),
    ];
    assert!(tool_result_needs_assistant_gap(
        &hidden_notice_then_assistant,
        0
    ));

    let user_follows = vec![
        ChatMessage::new("tool", "tool output"),
        ChatMessage::new("user", "next prompt"),
    ];
    assert!(!tool_result_needs_assistant_gap(&user_follows, 0));

    let consecutive_tools = vec![
        ChatMessage::new("tool", "first output"),
        ChatMessage::new("tool", "second output"),
    ];
    assert!(!tool_result_needs_assistant_gap(&consecutive_tools, 0));
}

#[test]
fn status_panels_render_minimal_inline() {
    use super::render_status_panel;

    let mut lines = Vec::new();
    render_status_panel("Session status: 5 messages", 80, false, &mut lines);

    assert_eq!(
        lines.len(),
        5,
        "boxed info status panel includes top/bottom borders & padding"
    );
    assert!(lines[0].spans[0].content.contains(">_ RustCode"));
    assert!(
        lines[2].spans[1]
            .content
            .contains("Session status: 5 messages")
    );

    let mut notice_lines = Vec::new();
    render_status_panel(
        "Notice: background task finished",
        80,
        false,
        &mut notice_lines,
    );

    assert_eq!(notice_lines.len(), 1, "ordinary notice panel skips header");
    assert!(notice_lines[0].spans[0].content.contains("  "));

    let mut loop_recovery_lines = Vec::new();
    render_status_panel(
        "[Evidence-based recovery: signal=no_new_information streak=4 action=view_file]. Use a different, evidence-producing next step; do not repeat the same unchanged read, no-result search, no-op edit, or failed command.\nThe previous tool action repeated without making progress. Tools remain enabled for one recovery attempt.",
        80,
        false,
        &mut loop_recovery_lines,
    );
    assert_eq!(loop_recovery_lines.len(), 1);
    assert_eq!(loop_recovery_lines[0].spans[0].content, "! ");
    assert_eq!(
        loop_recovery_lines[0].spans[1].content,
        "Repetitive tool actions detected — nudging agent to make progress"
    );

    let mut loop_abort_lines = Vec::new();
    render_status_panel(
        "[Evidence-based recovery: signal=no_new_information streak=6 action=view_file]. Use a different, evidence-producing next step.\nCRITICAL — you are stuck in a loop. Tools are now DISABLED for this turn. Do NOT emit any tool calls.",
        80,
        false,
        &mut loop_abort_lines,
    );
    assert_eq!(loop_abort_lines.len(), 1);
    assert_eq!(loop_abort_lines[0].spans[0].content, "! ");
    assert_eq!(
        loop_abort_lines[0].spans[1].content,
        "Repetitive tool loop detected — stopping tools and requesting final response"
    );

    let mut stream_recovery_lines = Vec::new();
    render_status_panel(
        "[Recoverable provider interruption: the response stream failed after a partial textual tool call. The partial response was saved, but no tool call from it was executed.]",
        80,
        false,
        &mut stream_recovery_lines,
    );
    assert_eq!(stream_recovery_lines.len(), 1);
    assert_eq!(stream_recovery_lines[0].spans[0].content, "! ");
    assert_eq!(
        stream_recovery_lines[0].spans[1].content,
        "Provider stream interrupted — output saved safely; no tool was replayed (send `continue` or use --resume)"
    );

    let mut yolo_enabled_lines = Vec::new();
    render_status_panel("YOLO mode enabled", 80, false, &mut yolo_enabled_lines);
    assert_eq!(yolo_enabled_lines.len(), 2);
    assert_eq!(
        yolo_enabled_lines[1].spans[1].content,
        " YOLO mode enabled "
    );

    let mut yolo_disabled_lines = Vec::new();
    render_status_panel("YOLO mode disabled", 80, false, &mut yolo_disabled_lines);
    assert_eq!(yolo_disabled_lines.len(), 2);
    assert_eq!(
        yolo_disabled_lines[1].spans[1].content,
        " YOLO mode disabled "
    );

    let mut cancelled_lines = Vec::new();
    render_status_panel(
        "[harness: turn stopped — cancelled]",
        80,
        false,
        &mut cancelled_lines,
    );
    assert_eq!(cancelled_lines.len(), 2);
    let cancelled = cancelled_lines[1].to_string();
    assert!(cancelled.starts_with("─ User Stopped ─"));
    assert!(!cancelled.contains('✕'));
}

#[test]
fn status_panel_help_box_lines_have_uniform_width() {
    use super::render_status_panel;

    let help_text = rustcode::controller::build_help_text();
    let mut lines = Vec::new();
    let total_width = 100u16;
    render_status_panel(&help_text, total_width, false, &mut lines);

    assert!(lines.len() > 10, "help text should render a full card");

    let expected_box_width = lines[0].width();
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(
            line.width(),
            expected_box_width,
            "line {i} ({:?}) must match box width {expected_box_width}",
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        );
    }
}

#[test]
fn command_panels_use_dynamic_titles_width_and_usage_section_spacing() {
    use super::render_status_panel;

    let mut usage_lines = Vec::new();
    render_status_panel(
        "Session usage:\nMessages: 2 user · 1 assistant · 0 tool calls\n  no token data yet - send a message first\n\nMonthly usage statistics:\n  2026-09: 1,024 prompt + 512 completion = 1,536 tokens (2 calls)",
        100,
        false,
        &mut usage_lines,
    );

    assert!(usage_lines[0].to_string().contains(">_ RustCode · Usage"));
    assert!(
        usage_lines[0].width() < 100,
        "a short command result should not stretch to the terminal edge: {usage_lines:?}"
    );
    let monthly_index = usage_lines
        .iter()
        .position(|line| line.to_string().contains("Monthly usage statistics"))
        .expect("monthly usage heading");
    assert!(monthly_index > 0);
    assert!(
        usage_lines[monthly_index - 1]
            .to_string()
            .trim_matches(['│', ' '])
            .is_empty(),
        "usage sections should have a visible blank row: {usage_lines:?}"
    );

    let mut info_lines = Vec::new();
    render_status_panel(
        "RustCode Info\nA short description",
        100,
        false,
        &mut info_lines,
    );
    assert!(info_lines[0].to_string().contains(">_ RustCode · Info"));
    assert!(
        info_lines
            .iter()
            .all(|line| !line.to_string().contains("RustCode Info")),
        "the panel title should replace the duplicated heading: {info_lines:?}"
    );
}

#[test]
fn new_chat_separator_spans_width_and_centers_label() {
    use super::push_new_chat_separator;
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    push_new_chat_separator(&mut lines, 40, false);

    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 40);
    assert_eq!(lines[1].spans[1].content, " ✨ NEW CHAT ");
    assert_eq!(lines[2].width(), 0);

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn resumed_session_separator_spans_width_and_centers_label() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    super::render_status_panel("Resumed session \"My Test Session\"", 60, false, &mut lines);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 60);
    assert_eq!(lines[1].spans[1].content, " Resumed Session ");
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.style.fg == Some(COLOR_TURN_SEPARATOR()))
    );

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn new_chat_started_separator_spans_width_without_emoji() {
    use unicode_width::UnicodeWidthStr;

    let mut lines = Vec::new();
    super::render_status_panel("New chat started", 60, false, &mut lines);

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].width(), 0);
    assert_eq!(lines[1].width(), 60);
    assert_eq!(lines[1].spans[1].content, " New Chat Started ");

    let left = lines[1].spans[0].content.width();
    let right = lines[1].spans[2].content.width();
    assert!((left as isize - right as isize).abs() <= 1);
}

#[test]
fn resumed_session_committed_block_has_top_and_bottom_padding() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "system",
        "Resumed session \"My Test Session\"",
    ));

    let block = super::render_committed_history_block(&state, 0, 60);
    assert_eq!(block.len(), 3);
    assert!(block[0].to_string().is_empty());
    assert!(block[1].to_string().contains("Resumed Session"));
    assert!(block[2].to_string().is_empty());
}

// Regression: a short transcript used to receive the entire remaining frame,
// pinning the input box to the bottom and leaving a large empty gap.
#[test]
fn conversation_area_height_fits_short_transcripts_and_caps_long_ones() {
    assert_eq!(conversation_area_height(8, 36), 8);
    assert_eq!(conversation_area_height(64, 36), 36);
    assert_eq!(conversation_area_height(0, 36), 0);
}

#[test]
fn harness_recovery_notices_are_hidden_from_transcript() {
    assert!(super::is_hidden_system_notice(
        "[harness: stopped after 10 tool round(s) — 4 consecutive malformed tool-call blocks the harness could not parse. The task is NOT complete. Review the transcript above; if the remaining work is still valid, resume it in a new turn.]"
    ));
    assert!(super::is_hidden_system_notice(
        "[Oversized response: only the first 1 tool calls were kept (use_skill); 1 more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]"
    ));
    assert!(!super::is_hidden_system_notice(
        "Notice: background task finished"
    ));
    assert!(super::is_hidden_system_notice(
        "[harness: turn stopped — cancelled]"
    ));
}

#[test]
fn deferred_tool_batch_notice_explains_scheduling_without_a_failure_warning() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "system",
        "[The model emitted 5 tool calls. 4 were executed this round; the remaining calls (get_status (call_123)) were not executed or scheduled. Reissue deferred calls only after reviewing the real results.]",
    ));

    let rendered = super::render_committed_history_block(&state, 0, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("deferred by the scheduler")),
        "rendered: {rendered:?}"
    );
    assert!(rendered.iter().all(|line| !line.starts_with("! ")));
    assert!(rendered.iter().all(|line| !line.contains("call_123")));
}

#[test]
fn session_command_uses_the_bordered_status_panel() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new(
        "system",
        "Session ID: session-123\nActive model: deepseek-v4.1-flash",
    ));
    let rendered = super::render_committed_history_block(&state, 0, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(rendered[0].contains(">_ RustCode · Session"));
    assert!(rendered.iter().any(|line| line.contains("session-123")));
}

#[test]
fn cancelled_turn_status_stays_out_of_the_chat() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "system",
        "[harness: turn stopped — cancelled]",
    ));

    let rendered = super::render_committed_history_block(&state, 0, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.is_empty());
}

#[test]
fn old_yolo_status_stays_out_of_the_chat() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "system",
        "YOLO mode enabled",
    ));

    let rendered = super::render_committed_history_block(&state, 0, 80)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert!(rendered.is_empty());
}

#[test]
fn assistant_oversized_response_notice_renders_empty_block() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "assistant",
        "[Oversized response: only the first 1 tool calls were kept (use_skill); 1 more were dropped. Anything the response claimed about their results was imagined — continue from the real results below.]",
    ));
    let block = super::render_committed_history_block(&state, 0, 80);
    assert!(block.is_empty());
}

#[test]
fn tool_action_formats_generic_args_and_omits_empty() {
    use super::format_pi_tool_action;

    let (action, arg) = format_pi_tool_action(
        "manage_task",
        &serde_json::json!({"Action": "status", "TaskId": "task-123"}),
        None,
    );
    assert_eq!(action, "ManageTask");
    assert_eq!(arg, "status task-123");

    let (action_list, arg_list) =
        format_pi_tool_action("manage_task", &serde_json::json!({"Action": "list"}), None);
    assert_eq!(action_list, "ManageTask");
    assert_eq!(arg_list, "list");

    let (action_bg, arg_bg) = format_pi_tool_action(
        "background_task",
        &serde_json::json!({"TaskId": "task-456"}),
        None,
    );
    assert_eq!(action_bg, "TaskDone");
    assert_eq!(arg_bg, "task-456");

    let (action2, arg2) = format_pi_tool_action("get_date", &serde_json::json!({}), None);
    assert_eq!(action2, "GetDate");
    assert_eq!(arg2, "");
}

#[test]
fn line_height_fast_path_matches_paragraph_wrap() {
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Wrap};

    let width = 80u16;
    let short_line = Line::from("Short text fits in viewport");
    let long_line = Line::from("A ".repeat(100));

    let short_w = short_line.width() as u16;
    let short_fast_h = if width == 0 || short_w <= width {
        1
    } else {
        Paragraph::new(vec![short_line.clone()])
            .wrap(Wrap { trim: false })
            .line_count(width) as u16
    };
    let short_expected_h = Paragraph::new(vec![short_line])
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    assert_eq!(short_fast_h, short_expected_h);

    let long_w = long_line.width() as u16;
    let long_fast_h = if width == 0 || long_w <= width {
        1
    } else {
        Paragraph::new(vec![long_line.clone()])
            .wrap(Wrap { trim: false })
            .line_count(width) as u16
    };
    let long_expected_h = Paragraph::new(vec![long_line])
        .wrap(Wrap { trim: false })
        .line_count(width) as u16;
    assert_eq!(long_fast_h, long_expected_h);
}

#[test]
fn footer_animation_pulse_center_reaches_both_edges() {
    let num_dots = 6;
    let pulse_centers_f = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 4.0, 3.0, 2.0, 1.0];
    assert_eq!(pulse_centers_f.first(), Some(&0.0));
    assert!(pulse_centers_f.contains(&(num_dots as f64 - 1.0)));
    assert_eq!(pulse_centers_f[5], 5.0);
}

#[test]
fn activity_status_labels_idle_and_working_states() {
    let state = RenderState::new();
    assert_eq!(activity_status_label(&render_snapshot(&state)), "Idle");
    assert_eq!(
        activity_status_line(&render_snapshot(&state), false, ROOMY_ACTIVITY_WIDTH)
            .spans
            .last()
            .unwrap()
            .content,
        " "
    );

    let mut streaming_state = RenderState::new();
    streaming_state.status = AppStatus::Streaming;
    assert_eq!(
        activity_status_label(&render_snapshot(&streaming_state)),
        "Working"
    );

    streaming_state.current_thought_started_at = Some(std::time::Instant::now());
    assert_eq!(
        activity_status_label(&render_snapshot(&streaming_state)),
        "Thinking"
    );
}

#[test]
fn streaming_decode_speed_is_displayed_in_composer_footer_not_activity() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(std::time::Instant::now());
    let mut tracker = rustcode::controller::StreamTracker::new();
    tracker.tokens_so_far = 8;
    tracker.record_chunk();
    state.stream_tracker = Some(tracker);

    let status =
        activity_status_line(&render_snapshot(&state), false, ROOMY_ACTIVITY_WIDTH).to_string();
    let rendered = render_state_to_text(&mut state, 100, 12);
    let footer = rendered
        .lines()
        .find(|line| line.contains("context left"))
        .expect("composer footer should be rendered");

    assert!(!status.contains("Tokens/s"), "{status}");
    assert!(footer.contains("Tokens/s: 80.0"), "{footer}");
    assert!(
        footer.find("Tokens/s: 80.0") < footer.find("context left"),
        "Tokens/s should appear immediately before context usage: {footer}"
    );
    assert!(status.contains("esc interrupt"), "{status}");

    let narrow = render_state_to_text(&mut state, 20, 12);
    let narrow_footer = narrow
        .lines()
        .find(|line| line.contains("context left"))
        .expect("context remains visible at narrow widths");
    assert!(!narrow_footer.contains("Tokens/s"), "{narrow_footer}");
}

/// The esc and steer-mode hints follow the same drop-don't-clip rule as the
/// footer hint: a clause that does not fit is omitted whole (#1529).
#[test]
fn activity_hints_are_dropped_rather_than_clipped_at_narrow_widths() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(std::time::Instant::now());
    let snapshot = render_snapshot(&state);

    let roomy = activity_status_line(&snapshot, false, ROOMY_ACTIVITY_WIDTH).to_string();
    assert!(roomy.contains(" · esc interrupt"), "{roomy}");

    // The label alone already overruns the row, so the hint has no room and is
    // omitted instead of being cut off mid-word.
    for width in [1, 4, 10] {
        let narrow = activity_status_line(&snapshot, false, width).to_string();
        assert!(
            !narrow.contains("esc"),
            "the esc hint must be dropped, not clipped, at width {width}: {narrow:?}"
        );
        assert!(
            !narrow.contains('…'),
            "no hint may end in a truncated ellipsis: {narrow:?}"
        );
    }
}

/// One row, one rule: the footer and the activity line share the clause-fitting
/// helper, and the percentage has the lower priority.
#[test]
fn hint_clauses_drop_from_the_tail_and_never_clip() {
    use super::composer_render::{
        COMMAND_COMPLETION_HINT_CLAUSES, COMPLETION_HINT_CLAUSES, fit_hint_clauses,
    };

    let clauses = &COMMAND_COMPLETION_HINT_CLAUSES;
    assert_eq!(
        fit_hint_clauses("  ", clauses, 200).as_deref(),
        Some("  ↑/↓ navigate · enter select · tab complete · esc dismiss")
    );
    // 44 columns hold the two leading clauses plus `tab complete`.
    assert_eq!(
        fit_hint_clauses("  ", clauses, 44).as_deref(),
        Some("  ↑/↓ navigate · enter select · tab complete")
    );
    assert_eq!(
        fit_hint_clauses("  ", clauses, 29).as_deref(),
        Some("  ↑/↓ navigate · enter select")
    );
    assert_eq!(
        fit_hint_clauses("  ", clauses, 28).as_deref(),
        Some("  ↑/↓ navigate")
    );
    assert_eq!(fit_hint_clauses("  ", clauses, 13), None);

    // The file-completion popup has no `tab complete` clause to begin with.
    assert_eq!(
        fit_hint_clauses("  ", &COMPLETION_HINT_CLAUSES, 200).as_deref(),
        Some("  ↑/↓ navigate · enter select · esc dismiss")
    );
    assert_eq!(
        fit_hint_clauses("  ", &COMPLETION_HINT_CLAUSES, 29).as_deref(),
        Some("  ↑/↓ navigate · enter select")
    );
    assert_eq!(
        fit_hint_clauses("  ", &COMPLETION_HINT_CLAUSES, 28).as_deref(),
        Some("  ↑/↓ navigate")
    );
    assert_eq!(fit_hint_clauses("  ", &COMPLETION_HINT_CLAUSES, 13), None);
}

/// The selection's copy key leads the footer, so it is the last clause a narrow
/// row drops (#1542).
///
/// The selection claims the copy chord outright, which makes it outrank the
/// completion popup's clauses and the passive metadata the hint replaces. The
/// order of [`footer_hint_clauses`] is therefore the footer's drop order, and
/// the order of these assertions is the claim.
#[test]
fn selection_copy_hint_leads_the_footer_drop_order() {
    use super::composer_render::{
        COMMAND_COMPLETION_HINT_CLAUSES, footer_hint_clauses, selection_hint_clauses,
    };

    let binding = rustcode::controller::copy_selection_binding();
    let copy: Vec<&str> = selection_hint_clauses().to_vec();
    assert_eq!(copy, vec![binding, "or right-click"]);

    // Nothing to say: no selection and no popup leaves the metadata row alone.
    assert_eq!(footer_hint_clauses(None, false), None);
    // A selection alone replaces the metadata with the copy chord.
    assert_eq!(
        footer_hint_clauses(None, true).as_deref(),
        Some(copy.as_slice())
    );
    // The popup alone keeps its own order untouched.
    assert_eq!(
        footer_hint_clauses(Some(&COMMAND_COMPLETION_HINT_CLAUSES), false).as_deref(),
        Some(&COMMAND_COMPLETION_HINT_CLAUSES[..])
    );
    // Both at once: the copy chord leads, so it is what survives a narrow row
    // and the popup's clauses are what degrade.
    assert_eq!(
        footer_hint_clauses(Some(&COMMAND_COMPLETION_HINT_CLAUSES), true),
        Some(
            [&binding, "or right-click"]
                .into_iter()
                .chain(COMMAND_COMPLETION_HINT_CLAUSES.iter().copied())
                .collect::<Vec<&'static str>>()
        )
    );
}

/// The copy chord degrades like every other footer clause: whole clauses, from
/// the tail, never clipped, and the context percentage yields rather than a key
/// the user can press (#1542).
#[test]
fn selection_copy_hint_degrades_by_content_at_narrow_widths() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let binding = rustcode::controller::copy_selection_binding();
    for (width, expected, keeps_percentage) in [
        (100u16, format!("  {binding} · or right-click"), true),
        // "or right-click" no longer fits in the 21 columns left beside
        // "100% context left  " (19), so it drops whole and the key keeps the
        // row. The percentage is still worth its 19 columns.
        (40, format!("  {binding}"), true),
        // 24 columns cannot fit the leading clause beside the percentage, so
        // the percentage is dropped rather than a key (#1529).
        (24, format!("  {binding}"), false),
    ] {
        let mut state = RenderState::new();
        let footer = footer_row_with_transcript_selection(&mut state, width, 20);
        assert!(
            footer.starts_with(&expected),
            "expected the footer to read {expected:?} at {width} columns, got {footer:?}"
        );
        assert!(
            !footer.contains('…'),
            "a hint must be omitted whole, never clipped with an ellipsis: {footer:?}"
        );
        assert_eq!(
            footer.chars().count(),
            width as usize,
            "the footer row must fill the viewport exactly once"
        );
        assert_eq!(
            footer.contains("context left"),
            keeps_percentage,
            "the context percentage is the passive metadata the copy key outranks: {footer:?}"
        );
        if keeps_percentage {
            assert!(
                footer.ends_with("100% context left  "),
                "the percentage stays right-aligned: {footer:?}"
            );
        }
    }

    // The same row with a completion popup open. On its own the popup keeps
    // "↑/↓ navigate" at 40 columns, so the copy key sitting there instead is the
    // drop order being applied rather than a row that happened to be narrow.
    let mut state = RenderState::new();
    state.input_buffer = "/".to_owned();
    state.cursor_position = 1;
    state.active_suggestion_index = Some(0);
    let footer = footer_row_with_transcript_selection(&mut state, 40, 20);
    assert!(
        footer.starts_with(&format!("  {binding}")),
        "the copy key leads the popup's clauses: {footer:?}"
    );
    assert!(
        !footer.contains("navigate"),
        "the popup's clauses degrade before the copy key does: {footer:?}"
    );
}

#[test]
fn background_terminal_activity_shows_management_hints_and_command() {
    let mut state = RenderState::new();
    // Unique session: the shared test config dir reuses last-active sessions
    // across tests, and task snapshots are session-scoped.
    state.active_session_id = "ui-background-footer-session".to_owned();
    state.waiting_for_background_terminal = true;
    let session_id = state.active_session_id.clone();
    let task_id = "ui-background-footer";
    let long_command = if cfg!(target_os = "windows") {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    spawn_background_task_for_test(task_id, &session_id, long_command).unwrap();
    // The engine projects the live task manager into the view; see
    // `controller::render_state`.
    state.background_tasks = rustcode::controller::background_task_snapshots(&session_id);
    let snapshot = render_snapshot(&state);
    state.waiting_for_background_terminal = false;
    let neutral_snapshot = render_snapshot(&state);
    rustcode::controller::stop_background_tasks(&session_id, None);

    let status = super::activity_status_line(&snapshot, false, ROOMY_ACTIVITY_WIDTH).to_string();
    assert!(status.contains("Idle"), "{status}");
    assert!(!status.contains("Waiting for background terminal"));
    assert!(!status.contains("esc to interrupt"));
    assert!(status.contains("⠋ 1 running ("), "{status}");
    assert!(status.contains(long_command), "{status}");
    assert!(status.contains("/ps · /stop"), "{status}");
    let neutral_status =
        super::activity_status_line(&neutral_snapshot, false, ROOMY_ACTIVITY_WIDTH).to_string();
    assert!(neutral_status.contains("Idle"));
    assert!(neutral_status.contains("1 running ("));
    assert!(!neutral_status.contains("Waiting for background terminal"));
    assert!(!neutral_status.contains("esc to interrupt"));

    let commands = super::background_command_lines(&snapshot);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].to_string(), format!("  └ {long_command}"));
    let live_tail = super::render_live_tail_snapshot(&snapshot, 120, 10)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!live_tail.contains("Idle"));
    assert!(!live_tail.contains("1 running ("));
    assert!(live_tail.contains(&format!("  └ {long_command}")));
    assert_eq!(
        rustcode::controller::background_command_label("cargo\n test\t--locked", 80),
        "cargo test --locked"
    );
}

#[test]
fn background_terminal_chip_compacts_more_than_three_tasks() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    // Unique session: the shared test config dir reuses last-active sessions
    // across tests, and task snapshots are session-scoped.
    state.active_session_id = "ui-background-chip-session".to_owned();
    let session_id = state.active_session_id.clone();
    let command = if cfg!(target_os = "windows") {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    for index in 0..4 {
        spawn_background_task_for_test(
            &format!("ui-background-chip-{index}"),
            &session_id,
            command,
        )
        .unwrap();
    }

    state.background_tasks = rustcode::controller::background_task_snapshots(&session_id);
    let snapshot = render_snapshot(&state);
    let summary = super::background_terminal_summary(&snapshot);
    rustcode::controller::stop_background_tasks(&session_id, None);

    assert!(summary.contains("⠋ 4 running ("), "{summary}");
    assert!(summary.contains("1 more"), "{summary}");
    assert!(summary.contains("/ps · /stop"), "{summary}");
}

#[test]
fn live_tool_activity_is_rendered_without_protocol_text() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time = Some(std::time::Instant::now());
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "call-1",
            None,
            "run_command",
            "Bash",
            "cargo test",
        ),
    );

    let line = super::activity_status_line(&render_snapshot(&state), false, ROOMY_ACTIVITY_WIDTH)
        .to_string();

    assert!(line.contains("Working"));
    assert!(line.contains("esc interrupt"));
    assert!(!line.contains("tool_calls"));
    assert!(!line.contains("Bash"));
    assert!(!line.contains("cargo test"));
}

#[test]
fn composer_footer_stays_compact_when_busy() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.model_name = "streaming-model".to_string();
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| {
            super::render(frame, &mut state);
        })
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(rendered.contains("streaming-model"));
    assert!(!rendered.contains("Enter message then press enter to queue"));
}

#[test]
fn live_history_cell_keeps_identical_invocations_visible_separately() {
    let calls = vec![
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "run_command",
            "Bash",
            "cargo test",
        ),
        rustcode::controller::LiveToolCall::new(
            "local:2",
            None,
            "run_command",
            "Bash",
            "cargo test",
        ),
    ];

    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert_eq!(rendered[0], "• Running");
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("Bash $ cargo test"))
            .count(),
        2,
        "the live cell must not deduplicate distinct invocation identities"
    );
}

#[test]
fn live_tool_cell_is_a_projection_not_history() {
    let mut state = RenderState::new();
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        ),
    );

    let text = super::render_live_tail(&state, 80, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("Exploring"));
    assert!(state.history.is_empty());
}

#[test]
fn live_tool_projection_does_not_hide_partial_assistant_stream() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "partial assistant response");
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        ),
    );

    let text = super::render_live_tail(&state, 80, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("src/main.rs"), "rendered: {text:?}");
    assert!(
        text.contains("partial assistant response"),
        "rendered: {text:?}"
    );
}

#[test]
fn live_tool_projection_hides_streamed_code_edit_call_syntax() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(
        &mut state,
        concat!(
            "The edit is in progress.\n\n```tool\n",
            r#"{"name":"replace_file_content","arguments":{"path":"src/main.rs","target_content":"old","replacement":"new"}}"#
        ),
    );
    assert!(
        rustcode_tool_protocol::parse_tool_call(
            &state.current_response,
            state.active_tool_protocol
        )
        .is_some()
    );
    assert!(
        super::scrollback::mutable_stream_text(&state.current_response).contains("target_content")
    );
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "replace_file_content",
            "Edit",
            "src/main.rs",
        ),
    );

    let text = super::render_live_tail(&state, 100, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("src/main.rs"), "rendered: {text:?}");
    assert!(!text.contains("target_content"), "rendered: {text:?}");
    assert!(!text.contains("replacement"), "rendered: {text:?}");
}

#[test]
fn live_tool_and_assistant_cells_update_and_clear_independently() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "partial response");
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        ),
    );
    let mut transcript = super::TranscriptState::default();

    let first =
        super::render_live_tail_with_transcript(&render_snapshot(&state), 80, 24, &mut transcript)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
    let repeated =
        super::render_live_tail_with_transcript(&render_snapshot(&state), 80, 24, &mut transcript)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
    assert_eq!(first, repeated, "unchanged projections must not duplicate");

    std::sync::Arc::make_mut(&mut state.live_tool_calls).clear();
    let assistant_only =
        super::render_live_tail_with_transcript(&render_snapshot(&state), 80, 24, &mut transcript)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
    assert!(assistant_only.contains("partial response"));
    assert!(!assistant_only.contains("src/main.rs"));

    set_current_response(&mut state, "");
    let tools_and_assistant_cleared =
        super::render_live_tail_with_transcript(&render_snapshot(&state), 80, 24, &mut transcript)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
    assert!(!tools_and_assistant_cleared.contains("partial response"));
    assert!(!tools_and_assistant_cleared.contains("src/main.rs"));
}

#[test]
fn live_exploration_batch_uses_exploring_when_one_call_is_executing() {
    let mut speculative =
        rustcode::controller::LiveToolCall::new("local:1", None, "grep", "Grep", "src/**/*.rs");
    speculative.execution_started = false;
    let executing = rustcode::controller::LiveToolCall::new(
        "local:2",
        None,
        "view_file",
        "Read",
        "src/main.rs",
    );

    let rendered = super::history_cell::render_live_tool_cell(&[speculative, executing], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Exploring");
    assert_eq!(rendered[1], "  └ Grep src/**/*.rs");
    assert_eq!(rendered[2], "    Read src/main.rs");
}

#[test]
fn mixed_live_exploration_and_action_batch_uses_running_heading() {
    let calls = vec![
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "view_file",
            "Read",
            "src/main.rs",
        ),
        rustcode::controller::LiveToolCall::new(
            "local:2",
            None,
            "write_to_file",
            "Writing",
            "src/main.rs",
        ),
    ];

    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Running");
    assert_eq!(rendered[1], "  └ Read src/main.rs");
    assert_eq!(rendered[2], "    Writing src/main.rs");
}

#[test]
fn live_mcp_calls_use_running_heading_when_one_call_is_executing() {
    let mut speculative = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "mcp__clockify__get_time",
        "ClockifyGetTime",
        "workspace",
    );
    speculative.execution_started = false;
    let executing = rustcode::controller::LiveToolCall::new(
        "local:2",
        None,
        "mcp__clockify__start_timer",
        "ClockifyStartTimer",
        "task-42",
    );

    let rendered = super::history_cell::render_live_tool_cell(&[speculative, executing], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Running");
    assert_eq!(rendered[1], "  └ ClockifyGetTime workspace");
    assert_eq!(rendered[2], "    ClockifyStartTimer task-42");
}

#[test]
fn single_live_generic_tool_is_nested_under_running_heading() {
    let call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "use_skill",
        "UseSkill",
        "release-automation",
    );
    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Running", "  └ UseSkill release-automation"]);
}

#[test]
fn speculative_live_tools_are_nested_under_preparing_heading() {
    let mut generic = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "use_skill",
        "UseSkill",
        "release-automation",
    );
    generic.execution_started = false;
    let mut command = rustcode::controller::LiveToolCall::new(
        "local:2",
        None,
        "run_command",
        "Bash",
        "cargo test",
    );
    command.execution_started = false;

    let rendered = super::history_cell::render_live_tool_cell(&[generic, command], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Queued",
            "  └ UseSkill release-automation",
            "    Bash $ cargo test"
        ]
    );
}

#[test]
fn speculative_file_write_uses_preparing_heading() {
    let mut call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "write_to_file",
        "Writing",
        "src/main.js",
    );
    call.execution_started = false;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Queued", "  └ Writing src/main.js"]);
}

#[test]
fn speculative_tool_without_target_is_not_rendered() {
    let mut call = rustcode::controller::LiveToolCall::new("local:1", None, "list", "List", "");
    call.execution_started = false;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false);

    assert!(rendered.is_empty());
}

#[test]
fn native_speculative_exploration_without_target_uses_preparing_heading() {
    let mut call =
        rustcode::controller::LiveToolCall::new("local:1", None, "grep", "Calling", "grep");
    call.execution_started = false;

    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(call);

    let text = super::render_live_tail(&state, 80, 24)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("• Queued"), "rendered: {text:?}");
    assert!(!text.contains("Calling grep"), "rendered: {text:?}");
    assert!(!text.contains("[TOOL_CALLS]"), "rendered: {text:?}");
}

#[test]
fn live_editing_tool_cell_shows_action_and_target_child() {
    let call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "replace_file_content",
        "Edit",
        "src/game/engine.ts",
    );
    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Running", "  └ Edit src/game/engine.ts"]);
}

#[test]
fn live_audio_generation_cell_shows_running_heading_and_output_path() {
    let arguments = serde_json::json!({
        "prompt": "a short balloon pop",
        "duration_seconds": 0.4,
        "output_path": "assets/audio/balloon-pop.wav"
    });
    let (action, target) =
        rustcode::controller::summarize_tool_call("generate_sound_effect", &arguments);
    let call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "generate_sound_effect",
        action,
        target,
    );

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Running",
            "  └ GenerateSoundEffect assets/audio/balloon-pop.wav"
        ]
    );
}

#[test]
fn live_video_render_cell_shows_progress() {
    let mut call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "render_video",
        "RenderVideo",
        "video-project.json",
    );
    call.output
        .push_back(rustcode::controller::LiveToolOutputChunk {
            stderr: true,
            text: "render progress: 42% (2.1s/5.0s)\n".to_owned(),
        });

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Running");
    assert_eq!(rendered[1], "  └ video-project.json");
    assert!(rendered[2].contains("render progress: 42% (2.1s/5.0s)"));
}

#[test]
fn live_batched_edits_with_casing_aliases_include_actions() {
    let calls = vec![
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "replace_file_content",
            "Edit",
            "src/game/engine.ts",
        ),
        rustcode::controller::LiveToolCall::new(
            "local:2",
            None,
            "WriteFile",
            "Write",
            "src/App.tsx",
        ),
    ];
    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Running",
            "  └ Edit src/game/engine.ts",
            "    Write src/App.tsx"
        ]
    );
}

#[test]
fn live_multiple_generic_tools_show_running_heading() {
    let calls = vec![
        rustcode::controller::LiveToolCall::new(
            "local:1",
            None,
            "clockify_timer",
            "ClockifyTimer",
            "start",
        ),
        rustcode::controller::LiveToolCall::new(
            "local:2",
            None,
            "notify_user",
            "NotifyUser",
            "done",
        ),
    ];
    let rendered = super::history_cell::render_live_tool_cell(&calls, 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        rendered,
        [
            "• Running",
            "  └ ClockifyTimer start",
            "    NotifyUser done"
        ]
    );
}

#[test]
fn live_command_cell_shows_bounded_stdout_stderr_and_omission() {
    let mut call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "run_command",
        "Bash",
        "cargo test",
    );
    call.output
        .push_back(rustcode::controller::LiveToolOutputChunk {
            stderr: false,
            text: (0..12).map(|line| format!("stdout {line}\n")).collect(),
        });
    call.output
        .push_back(rustcode::controller::LiveToolOutputChunk {
            stderr: true,
            text: "compiler error\n".to_owned(),
        });
    call.omitted_output_bytes = 4096;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    assert_eq!(rendered[0], "• Running");
    assert_eq!(rendered[1], "  └ Bash $ cargo test");
    assert!(rendered.iter().any(|line| line.contains("compiler error")));
    assert!(rendered.iter().any(|line| line.contains("lines")));
    assert!(rendered.iter().any(|line| line.contains("4K")));
    assert!(
        rendered.len() <= 7,
        "live output must fit a five-row body below its two-row header: {rendered:?}"
    );
}

#[test]
fn live_tool_output_shows_bash_only_at_low_verbosity() {
    use rustcode::controller::{LiveToolCall, LiveToolOutputChunk, Verbosity};

    for (name, action, target, payload, shown_at_low) in [
        (
            "run_command",
            "Bash",
            "printf output",
            "live-bash-payload",
            true,
        ),
        (
            "view_file",
            "Read",
            "src/main.rs",
            "live-read-payload",
            false,
        ),
        (
            "mcp_custom_tool",
            "Lookup",
            "query",
            "live-mcp-payload",
            false,
        ),
        (
            "use_skill",
            "UseSkill",
            "release-automation",
            "live-skill-payload",
            false,
        ),
    ] {
        let mut call = LiveToolCall::new("live:1", None, name, action, target);
        call.execution_started = true;
        call.output.push_back(LiveToolOutputChunk {
            stderr: false,
            text: payload.to_owned(),
        });

        let low = super::history_cell::render_live_tool_cell_with_verbosity(
            &[call.clone()],
            80,
            &Verbosity::Low,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert_eq!(
            low.contains(payload),
            shown_at_low,
            "low verbosity output for {name}: {low}"
        );

        let high = super::history_cell::render_live_tool_cell_with_verbosity(
            &[call],
            80,
            &Verbosity::High,
            false,
        )
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        assert!(
            !high.contains(payload),
            "high verbosity output for {name}: {high}"
        );
    }
}

#[test]
fn live_shell_output_wraps_japanese_and_keeps_omission_inside_five_rows() {
    use unicode_width::UnicodeWidthStr;

    let mut call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "run_command",
        "Bash",
        "spotify-cli p status",
    );
    call.output
        .push_back(rustcode::controller::LiveToolOutputChunk {
            stderr: false,
            text: format!(
                "{}\n{}\n",
                "日本語の長い状態".repeat(8),
                "次の状態".repeat(8)
            ),
        });
    call.omitted_output_bytes = 2048;

    let rendered = super::history_cell::render_live_tool_cell(&[call], 24, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(rendered.len() - 2 <= 5, "live body rows: {rendered:?}");
    assert!(rendered.iter().any(|line| line.contains('日')));
    assert!(rendered.iter().any(|line| line.contains("2K")));
    assert!(
        rendered.iter().all(|line| line.width() <= 24),
        "live Japanese output wraps by display width: {rendered:?}"
    );
}

#[test]
fn high_verbosity_live_command_cell_shows_only_the_invocation() {
    let mut call = rustcode::controller::LiveToolCall::new(
        "local:1",
        None,
        "run_command",
        "Bash",
        "cargo test",
    );
    call.output
        .push_back(rustcode::controller::LiveToolOutputChunk {
            stderr: false,
            text: "secret command output\n".to_owned(),
        });

    let rendered = super::history_cell::render_live_tool_cell_with_verbosity(
        &[call],
        80,
        &rustcode::controller::Verbosity::High,
        false,
    )
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>();

    assert_eq!(rendered, ["• Running", "  └ Bash $ cargo test"]);
    assert!(
        !rendered
            .iter()
            .any(|line| line.contains("secret command output"))
    );
}

#[test]
fn question_replaces_composer_with_borderless_bottom_pane() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new(
        "assistant",
        "The previous answer stays visible.",
    ));
    state.status = AppStatus::AwaitingQuestion;
    state.pending_question = Some(rustcode::controller::PendingQuestion::new(
        "Choose an option.".to_owned(),
        vec!["Option 1".to_owned(), "Option 2".to_owned()],
        false,
    ));
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state);
        })
        .unwrap();
    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(rendered.contains("Question"));
    assert!(rendered.contains("The previous answer stays visible."));
    assert!(
        !rendered.contains("unanswered"),
        "single questions show no chain chrome"
    );
    assert!(rendered.contains("› 1. Option 1"));
    assert!(rendered.contains("enter to submit answer"));
    assert!(!rendered.contains("Ask RustCode to do anything"));
    let rows = terminal
        .backend()
        .buffer()
        .content
        .chunks(80)
        .map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>();
    let question_row = rows
        .iter()
        .position(|row| row.contains("Question"))
        .expect("question panel row");
    assert!(
        rows[question_row..]
            .iter()
            .all(|row| !row.contains('╭') && !row.contains('╰')),
        "the question panel should stay borderless while the welcome panel remains in chat"
    );
}

#[test]
fn opening_question_keeps_transcript_rows_in_place_above_the_panel() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    for index in 0..12 {
        state.history.push(ChatMessage::new(
            "user",
            format!("conversation message {index}"),
        ));
    }
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    let mut transcript = TranscriptState::default();
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let before_cells = terminal.backend().buffer().content.clone();
    let before = (0..20)
        .map(|row| {
            (0..60)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    state.status = AppStatus::AwaitingQuestion;
    state.pending_question = Some(rustcode::controller::PendingQuestion::new(
        "Choose an option.".to_owned(),
        vec!["Option 1".to_owned(), "Option 2".to_owned()],
        false,
    ));
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let after = (0..20)
        .map(|row| {
            (0..60)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        &after[..6],
        &before[..6],
        "question should overlay the existing chat"
    );
    assert!(after.iter().any(|row| row.contains("Choose an option.")));

    state.status = AppStatus::Idle;
    state.pending_question = None;
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let restored = (0..20)
        .map(|row| {
            (0..60)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_eq!(restored, before, "closing the question restores the chat");
    assert_eq!(terminal.backend().buffer().content, before_cells);
}

#[test]
fn one_wheel_step_moves_a_wrapped_transcript_by_one_painted_row() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    for index in 0..12 {
        state.history.push(ChatMessage::new(
            "user",
            format!(
                "message {index} with enough words to wrap across the narrow transcript viewport"
            ),
        ));
    }
    let mut terminal = Terminal::new(TestBackend::new(36, 16)).unwrap();
    let mut transcript = TranscriptState::default();
    let mut input_area = ratatui::layout::Rect::default();
    terminal
        .draw(|frame| {
            input_area = render_with_transcript(frame, &mut state, &mut transcript).1;
        })
        .unwrap();
    let transcript_bottom = input_area.y.saturating_sub(1);
    let before = (0..transcript_bottom)
        .map(|row| {
            (0..36)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    transcript.scroll_up(1);
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let after = (0..transcript_bottom)
        .map(|row| {
            (0..36)
                .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_ne!(after, before, "wheel step should move the transcript");
    assert_eq!(after[2..], before[1..before.len() - 1]);
}

#[test]
fn a_wheel_tick_moves_three_painted_rows() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    assert_eq!(
        WHEEL_SCROLL_LINES, 3,
        "the wheel step matches the three lines per notch a terminal scrolls its own \
         scrollback, and a frame costs the same at 1 or 6 rows, so 3 is ~3x cheaper \
         per line scrolled than 1"
    );

    let mut state = RenderState::new();
    for index in 0..12 {
        state.history.push(ChatMessage::new(
            "user",
            format!(
                "message {index} with enough words to wrap across the narrow transcript viewport"
            ),
        ));
    }
    let mut terminal = Terminal::new(TestBackend::new(36, 16)).unwrap();
    let mut transcript = TranscriptState::default();
    let mut input_area = ratatui::layout::Rect::default();
    terminal
        .draw(|frame| {
            input_area = render_with_transcript(frame, &mut state, &mut transcript).1;
        })
        .unwrap();
    // The row immediately above input is reserved for the return-to-latest
    // control; compare transcript rows independently of that composer slot.
    let transcript_bottom = input_area.y.saturating_sub(1);
    let painted = |terminal: &Terminal<TestBackend>| {
        (0..transcript_bottom)
            .map(|row| {
                (0..36)
                    .map(|column| terminal.backend().buffer()[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    let before = painted(&terminal);

    transcript.scroll_up(WHEEL_SCROLL_LINES);
    terminal
        .draw(|frame| {
            render_with_transcript(frame, &mut state, &mut transcript);
        })
        .unwrap();
    let after = painted(&terminal);

    assert_eq!(transcript.scroll_rows(), WHEEL_SCROLL_LINES);
    assert_ne!(after, before, "a wheel tick should move the transcript");
    assert_eq!(
        &after[WHEEL_SCROLL_LINES + 1..],
        &before[1..before.len() - WHEEL_SCROLL_LINES],
        "one wheel tick should shift the painted viewport by {WHEEL_SCROLL_LINES} rows, \
         soft-wrapped rows included"
    );
}

#[test]
fn scrolled_transcript_keeps_its_reading_rows_when_history_grows() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    for index in 0..24 {
        state
            .history
            .push(ChatMessage::new("user", format!("reading item {index:02}")));
    }
    let mut transcript = TranscriptState::default();
    let visible = |state: &RenderState, transcript: &mut TranscriptState, height| {
        let snapshot = render_snapshot(state);
        super::conversation_render::render_visible_conversation_with_transcript(
            &snapshot, 64, height, transcript,
        )
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
    };
    let _ = visible(&state, &mut transcript, 8);
    transcript.scroll_up(6);
    let before = visible(&state, &mut transcript, 8);
    set_current_response(&mut state, "partial streaming answer");
    assert_eq!(visible(&state, &mut transcript, 8), before);
    set_current_response(&mut state, "partial streaming answer with more text");
    assert_eq!(visible(&state, &mut transcript, 8), before);
    set_current_response(&mut state, "");
    state
        .history
        .push(ChatMessage::new("tool", "new tool result"));
    state
        .history
        .push(ChatMessage::new("assistant", "new committed output"));
    let after = visible(&state, &mut transcript, 8);
    assert_eq!(
        after, before,
        "appended output must not move the reading viewport"
    );

    let _ = visible(&state, &mut transcript, 0);
    state
        .history
        .push(ChatMessage::new("assistant", "output while hidden"));
    assert_eq!(visible(&state, &mut transcript, 8), before);

    transcript.scroll_down(usize::MAX);
    let _ = visible(&state, &mut transcript, 8);
    state
        .history
        .push(ChatMessage::new("assistant", "newer committed output"));
    let following = visible(&state, &mut transcript, 8).join("\n");
    assert!(following.contains("newer committed output"));
}

#[test]
fn first_committed_response_keeps_scrolled_welcome_until_follow_resumes() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    let mut transcript = TranscriptState::default();
    let visible = |state: &RenderState, transcript: &mut TranscriptState| {
        let snapshot = render_snapshot(state);
        super::conversation_render::render_visible_conversation_with_transcript(
            &snapshot, 64, 8, transcript,
        )
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
    };
    let _ = visible(&state, &mut transcript);
    transcript.scroll_up(3);
    let before = visible(&state, &mut transcript);
    assert!(transcript.scroll_rows() > 0);
    state
        .history
        .push(ChatMessage::new("assistant", "first committed response"));
    let after = visible(&state, &mut transcript);
    assert_eq!(
        after, before,
        "first response should not move the welcome reading position"
    );
    transcript.scroll_down(usize::MAX);
    assert!(visible(&state, &mut transcript).contains("first committed response"));
}

#[test]
fn scrolled_transcript_keeps_top_row_when_viewport_shrinks() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    for index in 0..24 {
        state
            .history
            .push(ChatMessage::new("user", format!("reading item {index:02}")));
    }
    let mut transcript = TranscriptState::default();
    let snapshot = render_snapshot(&state);
    let _ = super::conversation_render::render_visible_conversation_with_transcript(
        &snapshot,
        64,
        8,
        &mut transcript,
    );
    transcript.scroll_up(6);
    let before = super::conversation_render::render_visible_conversation_with_transcript(
        &snapshot,
        64,
        8,
        &mut transcript,
    );
    let after = super::conversation_render::render_visible_conversation_with_transcript(
        &snapshot,
        64,
        6,
        &mut transcript,
    );
    let text = |lines: Vec<ratatui::text::Line<'static>>| {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(text(after), text(before)[..6]);
}

#[test]
fn active_transcript_cell_updates_in_place_and_clears_without_history() {
    let mut transcript = super::TranscriptState::default();

    transcript.set_assistant("first paragraph", false, None, None, None);
    let first_revision = transcript.revision();
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_assistant("first paragraph\n\nsecond", true, None, None, None);
    assert!(transcript.revision() > first_revision);
    assert!(!transcript.display_lines(80).is_empty());

    transcript.set_tools(&[rustcode::controller::LiveToolCall::new(
        "call-1",
        Some("native-1".to_owned()),
        "run_command",
        "Bash",
        "cargo test",
    )]);
    assert!(transcript.revision() > first_revision);
    assert!(
        transcript
            .display_lines(80)
            .iter()
            .any(|line| line.to_string().contains("cargo test"))
    );

    transcript.clear();
    assert!(transcript.display_lines(80).is_empty());
}

#[test]
fn action_required_status_wins_over_a_live_question_tool() {
    let mut state = RenderState::new();
    state.status = AppStatus::AwaitingQuestion;
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "question",
            None,
            "ask_question",
            "AskQuestion",
            "continue?",
        ),
    );

    assert_eq!(
        super::activity_status_label(&render_snapshot(&state)),
        "Action Required"
    );
}

#[test]
fn split_stable_rows_keeps_only_the_incomplete_suffix_live() {
    let (stable, tail) = super::scrollback::split_stable_rows("first\nsecond\nthird");

    assert_eq!(stable, vec!["first", "second"]);
    assert_eq!(tail, "third");
}

#[test]
fn transcript_cursor_never_recommits_history_or_stream_rows() {
    let mut cursor = super::scrollback::TranscriptCursor::default();

    assert_eq!(cursor.take_history_range(3), 0..3);
    assert_eq!(cursor.take_history_range(3), 3..3);
    assert_eq!(cursor.take_stable_stream("alpha\n\nbeta"), vec!["alpha"]);
    assert!(cursor.take_stable_stream("alpha\n\nbeta").is_empty());
}

#[test]
fn transcript_cursor_retries_pending_content_until_acknowledged() {
    let mut cursor = super::scrollback::TranscriptCursor::default();

    assert_eq!(cursor.pending_history_range(2), 0..2);
    assert_eq!(cursor.pending_history_range(2), 0..2);
    cursor.commit_history_through(2);
    assert_eq!(cursor.pending_history_range(2), 2..2);

    assert_eq!(cursor.pending_stable_stream("line\n\ntail"), vec!["line"]);
    assert_eq!(cursor.pending_stable_stream("line\n\ntail"), vec!["line"]);
    cursor.commit_stable_stream("line\n\n");
    assert!(cursor.pending_stable_stream("line\n\ntail").is_empty());
}

#[test]
fn transcript_cursor_reset_replays_history_after_resize() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_history_through(4);
    cursor.commit_stable_stream("already rendered\n");

    cursor.reset();

    assert_eq!(cursor.pending_history_range(4), 0..4);
    assert_eq!(
        cursor.pending_stable_stream("already rendered\n\nnext row"),
        vec!["already rendered"]
    );
}

#[test]
fn transcript_cursor_holds_thought_stream_until_finalized() {
    let cursor = super::scrollback::TranscriptCursor::default();

    assert!(
        cursor
            .pending_stable_stream("<think>\nPlanning\n")
            .is_empty()
    );
    assert!(
        cursor
            .pending_stable_stream("thoughtPlanning the response\n")
            .is_empty()
    );
}

#[test]
fn transcript_cursor_keeps_an_incomplete_code_fence_together() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let stream = "intro\n\n```rust\nfn main() {\n";

    assert_eq!(cursor.pending_stable_source(stream), "intro\n\n".to_owned());
    assert_eq!(cursor.pending_stable_stream(stream), vec!["intro"]);
    assert_eq!(
        super::scrollback::mutable_stream_text(stream),
        "```rust\nfn main() {\n".to_owned()
    );

    cursor.commit_stable_stream("intro\n\n");
    assert!(cursor.pending_stable_stream(stream).is_empty());

    let completed = "intro\n\n```rust\nfn main() {}\n```\n";
    assert_eq!(cursor.pending_stable_source(completed), String::new());
}

#[test]
fn transcript_cursor_keeps_streamed_tables_mutable_until_finalization() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let header = "intro\n\n| Name | Value |\n";
    let with_delimiter = "intro\n\n| Name | Value |\n| --- | --- |\n";
    let with_row = "intro\n\n| Name | Value |\n| --- | --- |\n| one | two |\n";

    assert_eq!(cursor.pending_stable_source(header), "intro\n\n");
    assert_eq!(
        super::scrollback::mutable_stream_text(header),
        "| Name | Value |\n"
    );
    cursor.commit_stable_stream("intro\n\n");

    assert!(cursor.pending_stable_stream(with_delimiter).is_empty());
    assert_eq!(
        super::scrollback::mutable_stream_text(with_row),
        "| Name | Value |\n| --- | --- |\n| one | two |\n"
    );
    assert!(cursor.pending_stable_stream(with_row).is_empty());

    let remainder = cursor
        .take_final_stream_remainder(with_row)
        .expect("stream prefix should be acknowledged");
    assert_eq!(remainder, with_row.strip_prefix("intro\n\n").unwrap());

    cursor.reset();
    assert_eq!(cursor.pending_stable_source(with_row), "intro\n\n");
}

#[test]
fn transcript_cursor_does_not_hold_pipe_text_without_a_table_delimiter() {
    let cursor = super::scrollback::TranscriptCursor::default();
    let stream = "A | B\nThis is ordinary prose\n";

    assert!(cursor.pending_stable_stream(stream).is_empty());
}

#[test]
fn transcript_cursor_releases_completed_fence_and_replays_after_resize() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let before_close = "intro\n````rust\nlet text = \"```\";\n";

    assert_eq!(
        cursor.pending_stable_source(before_close),
        "intro\n".to_owned()
    );
    assert_eq!(
        super::scrollback::mutable_stream_text(before_close),
        "````rust\nlet text = \"```\";\n".to_owned()
    );
    cursor.commit_stable_stream("intro\n");

    let after_close = "intro\n````rust\nlet text = \"```\";\n````\nnext row";
    assert_eq!(
        cursor.pending_stable_source(after_close),
        "````rust\nlet text = \"```\";\n````\n".to_owned()
    );
    assert_eq!(
        super::scrollback::mutable_stream_text(after_close),
        "next row".to_owned()
    );

    cursor.commit_stable_stream("````rust\nlet text = \"```\";\n````\n");
    cursor.reset();
    assert_eq!(
        cursor.pending_stable_source(after_close),
        "intro\n````rust\nlet text = \"```\";\n````\n".to_owned()
    );
}

#[test]
fn live_tail_excludes_committed_history() {
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("assistant", "old completed answer"));
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "stable line\nunclosed tail");

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(!text.contains("Working"));
    assert!(text.contains("unclosed tail"));
    assert!(!text.contains("old completed answer"));
}

#[test]
fn reasoning_prefixed_stream_keeps_completed_answer_lines_live() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(
        &mut state,
        "<think>\nPlanning\n</think>\n\nFirst answer line\nSecond answer line",
    );

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(
        text.contains("First answer line"),
        "completed answer rows must remain visible while the next row streams: {text:?}"
    );
    assert!(text.contains("Second answer line"));
}

#[test]
fn bare_thought_stream_stays_in_the_compact_reasoning_preview() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "thoughtPlanning the response\n");

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(text.contains("Thought"));
    assert!(text.contains("Planning the response"));
    assert!(!text.contains("thoughtPlanning"));
}

#[test]
fn assistant_messages_use_a_gutter_after_soft_reflow() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "one two three four five six seven",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 20,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let prose: Vec<_> = lines.iter().filter(|line| !line.spans.is_empty()).collect();
    assert_eq!(prose[0].spans[0].content, "• ");
    let first_line = prose[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(first_line.contains("one two"));
    assert_eq!(prose[1].spans[0].content, "  ");
}

#[test]
fn streamed_assistant_chunks_only_bullet_the_first_chunk() {
    let state = RenderState::new();
    let first = super::render_committed_assistant_chunk(&state, "first line\n", 80, false);
    let continuation = super::render_committed_assistant_chunk(&state, "second line\n", 80, true);

    assert_eq!(first[0].spans[0].content, "• ");
    assert_eq!(continuation[0].spans[0].content, "  ");
}

#[test]
fn assistant_message_uses_one_gutter_across_paragraphs() {
    use super::{AssistantRenderOptions, render_assistant_message};

    let mut lines = Vec::new();
    let mut copies = Vec::new();
    render_assistant_message(
        "first paragraph\n\n```text\ncode\n```\n\nsecond paragraph",
        &mut lines,
        &mut copies,
        AssistantRenderOptions {
            token_usage: None,
            response_time_ms: None,
            thought_time_ms: None,
            thought_tokens: None,
            is_generating: false,
            viewport_width: 80,
            show_picker: false,
            last_copy_text: None,
        },
    );

    let prefixes = lines
        .iter()
        .filter(|line| !line.spans.is_empty())
        .filter_map(|line| line.spans.first())
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>();

    assert_eq!(prefixes.first(), Some(&"• "));
    assert!(prefixes.iter().skip(1).all(|prefix| *prefix == "  "));
}

#[test]
fn committed_user_messages_keep_regular_body_text() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("user", "inspect the parser"));

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block[1].spans[0].content, "› ");
    assert_eq!(block[1].width(), 80);
    assert!(
        block[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        block[1]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        block[2]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(super::COLOR_PANEL()))
    );
    assert!(
        !block[1].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
}

#[test]
fn committed_user_message_has_trailing_blank_line() {
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("user", "check latest 10 commits"));

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block.len(), 4);
    assert_eq!(block[0].width(), 80);
    assert_eq!(block[2].width(), 80);
    assert_eq!(
        block[0]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        ""
    );
    assert_eq!(
        block[1]
            .spans
            .iter()
            .skip(1)
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        format!("{:<78}", "check latest 10 commits")
    );
    assert!(block[3].spans.is_empty());
}

#[test]
fn committed_assistant_message_has_one_trailing_separator() {
    let state = RenderState::new();

    let block = super::render_committed_assistant_text(&state, "Finished.", 80);

    assert_eq!(block.len(), 2);
    assert_eq!(block[0].spans[0].content, "• ");
    assert!(block[1].spans.is_empty());
}

#[test]
fn conversation_recap_renders_as_compact_labeled_block() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new(
            "assistant",
            "The implementation is complete; cargo test passes and the next step is review.",
        )
        .as_conversation_recap(),
    );

    let rendered = super::render_committed_history_block(&state, 0, 80);
    let text = rendered
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert!(text[0].starts_with("  ↳ Recap: "), "{text:?}");
    assert!(!text.iter().any(|line| line.contains("─")));
    assert!(
        rendered
            .iter()
            .flat_map(|line| &line.spans)
            .all(|span| span.style.add_modifier.contains(Modifier::ITALIC))
    );
    assert!(text.concat().contains("implementation is complete"));
}

#[test]
fn conversation_recap_renders_sanitized_plain_text() {
    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new(
            "assistant",
            "### 1. Done\n- **Fixed** `src/main.rs`\n\n### 2. Next\n- Run tests",
        )
        .as_conversation_recap(),
    );

    let rendered = super::render_committed_history_block(&state, 0, 80);
    let text = rendered
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Fixed src/main.rs Run tests"), "{text}");
    assert!(!text.contains("###"), "{text}");
    assert!(!text.contains("**"), "{text}");
    assert!(!text.contains('`'), "{text}");
}

#[test]
fn conversation_recap_wraps_inside_its_message_gutter() {
    let mut state = RenderState::new();
    state.history.push(
        ChatMessage::new(
            "assistant",
            "The recap remains aligned while its long message wraps across multiple lines.",
        )
        .as_conversation_recap(),
    );

    let rendered = super::render_committed_history_block(&state, 0, 32);
    let text = rendered
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert!(text.len() > 2, "recap fixture must wrap: {text:?}");
    assert!(text[0].starts_with("  ↳ Recap: "));
    assert!(
        text.iter()
            .skip(1)
            .all(|line| line.starts_with("           "))
    );
    assert!(rendered.iter().all(|line| line.width() <= 30));
}

#[test]
fn committed_assistant_message_uses_saved_thought_metrics() {
    let mut state = RenderState::new();
    let mut message = ChatMessage::new("assistant", "<think>Planning.</think>Finished.");
    message.thought_time_ms = Some(1250);
    message.thought_tokens = Some(42);
    state.history.push(message);

    let block = super::render_committed_history_block(&state, 0, 80);

    assert_eq!(block[0].spans[1].content, "Thought for 1.2s, 42 tokens");
}

#[test]
fn committed_thought_only_message_has_a_separator_before_tools() {
    let mut state = RenderState::new();
    let mut message = ChatMessage::new(
        "assistant",
        "<think>Find the Rust files before reading them.</think>",
    );
    message.thought_time_ms = Some(718);
    message.thought_tokens = Some(31);
    state.history.push(message);
    state
        .history
        .push(ChatMessage::new("tool", "glob: src/main.rs"));

    let thought = super::render_committed_history_block(&state, 0, 80);
    let tool = super::render_committed_history_block(&state, 1, 80);

    assert!(
        thought[0]
            .to_string()
            .contains("Thought for 718ms, 31 tokens")
    );
    assert!(thought.last().is_some_and(|line| line.spans.is_empty()));
    assert!(tool.first().is_some_and(|line| !line.spans.is_empty()));
}

#[test]
fn live_tail_uses_formatted_working_status() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;

    let text = super::render_live_tail(&state, 80, 24)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert!(!text.contains("• Working"));
    assert!(!text.contains("esc interrupt"));
    assert!(!text.contains("Working..."));
}

#[test]
fn live_tail_shows_only_the_running_indicator_when_nothing_has_been_produced() {
    // A running turn with no text, no tools and no history must still show
    // that work is happening instead of an empty chat (#1626 feedback).
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.config.reduced_motion = true;

    let mut transcript = TranscriptState::default();
    let mut terminal =
        crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(80, 12))
            .unwrap();
    terminal
        .draw(|frame| {
            let snapshot = render_snapshot(&state);
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();

    let rows = (0..12)
        .map(|y| {
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let indicator = rows
        .iter()
        .find(|row| row.contains(&state.model_name))
        .unwrap_or_else(|| panic!("a running turn must show its model: {rows:?}"));
    assert!(
        indicator.trim_start().starts_with('•'),
        "indicator must lead with the spinner: {indicator:?}"
    );
}

#[test]
fn visible_streaming_text_keeps_working_status_until_completion() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    set_current_response(
        &mut state,
        (1..=10)
            .map(|line| format!("streamed line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let lines = super::render_live_tail(&state, 30, 24);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();
    let text = rendered.join(" ");

    assert!(
        text.contains("streamed line 1"),
        "streaming lines: {rendered:?}"
    );
    assert!(text.contains("line 10"), "streaming lines: {rendered:?}");
    assert!(rendered.len() > 5, "streaming lines: {rendered:?}");
    assert!(!rendered.iter().any(|line| line.contains("Working")));
    assert!(lines.last().is_some_and(|line| !line.spans.is_empty()));
}

#[test]
fn consecutive_thought_blocks_have_a_blank_line_gap() {
    let mut lines = Vec::new();
    let mut copy_clicks = Vec::new();
    let options = super::AssistantRenderOptions {
        token_usage: None,
        response_time_ms: None,
        thought_time_ms: Some(1500),
        thought_tokens: Some(100),
        is_generating: false,
        viewport_width: 80,
        show_picker: false,
        last_copy_text: None,
    };

    super::render_assistant_message(
        "<think>\nFirst thought\n</think>\nFirst response",
        &mut lines,
        &mut copy_clicks,
        options,
    );

    let options2 = super::AssistantRenderOptions {
        token_usage: None,
        response_time_ms: None,
        thought_time_ms: Some(2000),
        thought_tokens: Some(150),
        is_generating: false,
        viewport_width: 80,
        show_picker: false,
        last_copy_text: None,
    };

    super::render_assistant_message(
        "<think>\nSecond thought\n</think>\nSecond response",
        &mut lines,
        &mut copy_clicks,
        options2,
    );

    let thought_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.spans.iter().any(|s| s.content.contains("Thought for")))
        .map(|(i, _)| i)
        .collect();

    assert_eq!(thought_indices.len(), 2);
    assert!(lines[thought_indices[1] - 1].spans.is_empty());
}

#[test]
fn active_turn_uses_only_the_history_separator_above_working() {
    let mut state = RenderState::new();
    assert_eq!(
        super::live_surface_padding(&render_snapshot(&state)),
        (1, 1)
    );

    state.status = AppStatus::Streaming;
    assert_eq!(
        super::live_surface_padding(&render_snapshot(&state)),
        (0, 1)
    );
}

#[test]
fn activity_spacing_adds_gaps_only_when_active_and_tall_enough() {
    // Idle never gains a gap.
    assert_eq!(super::activity_spacing(false, 24, 10, 3), (0, 0));
    // Active with ample height gets one row above and below.
    assert_eq!(super::activity_spacing(true, 24, 10, 3), (1, 1));
    // Short terminals drop the optional gaps first.
    assert_eq!(super::activity_spacing(true, 10, 9, 3), (0, 0));
    assert_eq!(super::activity_spacing(true, 12, 10, 3), (0, 0));
}

#[test]
fn streaming_layout_keeps_composer_and_footer_visible_with_gaps() {
    let mut idle = RenderState::new();
    let idle_text = render_state_to_text(&mut idle, 80, 20);
    assert!(idle_text.contains("context left"));

    let mut streaming = RenderState::new();
    streaming.status = AppStatus::Streaming;
    let streaming_text = render_state_to_text(&mut streaming, 80, 20);
    assert!(!streaming_text.contains("Working · "));
    assert!(!streaming_text.contains("esc interrupt"));
    assert!(streaming_text.contains("context left"));

    // Constrained height must not clip composer/footer for spacing.
    let mut short = RenderState::new();
    short.status = AppStatus::Streaming;
    let short_text = render_state_to_text(&mut short, 80, 8);
    assert!(short_text.contains("context left"));
}

#[test]
fn empty_composer_has_painted_padding_and_external_model_footer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");

    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 12)).unwrap();
    let mut input_area = ratatui::layout::Rect::default();
    terminal
        .draw(|frame| {
            input_area = super::render(frame, &mut state).1;
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let prompt_row = (0..12).find(|y| {
        (0..100)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("Ask RustCode to do anything")
    });
    let bottom_border_row = (0..12).find(|y| {
        (0..100)
            .map(|x| buffer[(x, *y)].symbol())
            .collect::<String>()
            .contains("context left")
    });

    let prompt_row = prompt_row.expect("composer prompt should be rendered");
    let footer_row = bottom_border_row.expect("composer footer should be rendered");
    assert_eq!(
        Some(input_area.y),
        Some(prompt_row - 1),
        "shutdown should know where the transient composer begins"
    );
    assert_eq!(footer_row, prompt_row + 2);
    let footer = (0..100)
        .map(|x| buffer[(x, footer_row)].symbol())
        .collect::<String>();
    assert!(
        footer.contains(&state.model_name),
        "composer footer: {footer:?}"
    );
    assert!(
        !footer.contains("? for shortcuts"),
        "composer footer: {footer:?}"
    );
    assert!(
        footer.contains(" · "),
        "composer footer should include model/location separators: {footer:?}"
    );
    assert_eq!(buffer[(0, prompt_row - 1)].bg, COLOR_PANEL());
    assert_eq!(buffer[(0, prompt_row + 1)].bg, COLOR_PANEL());
    assert_eq!(buffer[(0, footer_row)].bg, COLOR_BG());
    assert_eq!(buffer[(99, prompt_row)].bg, COLOR_PANEL());
}

#[test]
fn armed_ctrl_c_is_visible_in_the_production_composer_footer() {
    let mut state = RenderState::new();
    state.ctrl_c_exit_armed = true;

    let rendered = render_state_to_text(&mut state, 100, 12);

    assert!(rendered.contains("⚠ Press Ctrl+C again to exit"));
    let footer = rendered
        .lines()
        .find(|line| line.contains("Press Ctrl+C again to exit"))
        .expect("exit warning should be rendered in the composer footer");
    assert!(
        footer.starts_with("  ⚠ Press Ctrl+C again to exit"),
        "exit warning should be left-aligned: {footer:?}"
    );
    assert!(
        footer.ends_with("100% context left  "),
        "context usage should remain right-aligned: {footer:?}"
    );

    // The warning is clipped safely, rather than causing a layout overflow,
    // on terminals narrower than the complete message.
    let narrow = render_state_to_text(&mut state, 24, 12);
    assert!(narrow.contains('⚠'));
}

#[test]
fn armed_ctrl_c_keeps_the_copy_result_from_the_same_press_visible() {
    let mut state = RenderState::new();
    state.ctrl_c_exit_armed = true;
    state
        .transient_notice
        .replace("Copied selection to clipboard".to_owned());

    let rendered = render_state_to_text(&mut state, 100, 12);

    assert!(
        rendered.contains("Copied selection to clipboard · ⚠ Press Ctrl+C again to exit"),
        "one press can copy and arm, so both answers belong on screen: {rendered:?}"
    );

    // An unrelated notice must not be presented as a copy result.
    let mut other = RenderState::new();
    other.ctrl_c_exit_armed = true;
    other
        .transient_notice
        .replace("Model switched to something".to_owned());
    assert!(
        !render_state_to_text(&mut other, 100, 12).contains("Model switched to something"),
        "an unrelated notice should not crowd the pending-exit warning"
    );
}

#[test]
fn composer_footer_shows_path_and_truncates_long_branch() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.cwd_and_branch =
        "~/code/rustcode:feature/a-branch-name-that-is-definitely-too-long".to_string();
    let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
    terminal
        .draw(|frame| {
            super::render(frame, &mut state);
        })
        .unwrap();

    let footer_row = (0..12)
        .find(|row| {
            (0..120)
                .map(|column| terminal.backend().buffer()[(column, *row)].symbol())
                .collect::<String>()
                .contains("context left")
        })
        .expect("composer footer should be rendered");
    let footer = (0..120)
        .map(|column| terminal.backend().buffer()[(column, footer_row)].symbol())
        .collect::<String>();
    assert!(footer.contains("~/code/rustcode"));
    assert!(footer.contains("feature/a-branch-name-t…"));
    assert!(
        !footer.contains("definitely-too-long"),
        "footer should not contain the untruncated branch: {footer:?}"
    );
}

#[test]
fn composer_footer_is_hidden_while_a_picker_is_open() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.show_model_picker = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
    terminal
        .draw(|frame| {
            super::render(frame, &mut state);
        })
        .unwrap();

    let rendered = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(!rendered.contains("context left"));
}

#[test]
fn codex_shimmer_moves_a_visible_gradient_across_working() {
    let early = super::shimmer_spans_at("Working", std::time::Duration::from_millis(850));
    let later = super::shimmer_spans_at("Working", std::time::Duration::from_millis(1100));
    let early_colors = early.iter().map(|span| span.style.fg).collect::<Vec<_>>();
    let later_colors = later.iter().map(|span| span.style.fg).collect::<Vec<_>>();

    assert!(
        early_colors.iter().any(|color| *color != early_colors[0]),
        "a visible frame must not paint the whole word one color: {early_colors:?}"
    );
    assert!(
        later_colors.iter().any(|color| *color != later_colors[0]),
        "a visible frame must not paint the whole word one color: {later_colors:?}"
    );
    assert_ne!(
        early_colors, later_colors,
        "the gradient must travel over time"
    );
}

#[test]
fn reduced_motion_renders_the_activity_label_without_a_sweep() {
    let animated = super::shimmer_spans("Working", false, false);
    let still = super::shimmer_spans("Working", false, true);
    assert_ne!(animated.len(), 1, "the default label still animates");

    let still_text: String = still.iter().map(|span| span.to_string()).collect();
    assert_eq!(still_text, "Working");
    assert!(
        still.iter().all(|span| span.style.fg == still[0].style.fg),
        "a reduced-motion label must not encode a gradient: {still:?}"
    );
}

#[test]
fn reduced_motion_is_read_from_the_active_config() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = rustcode::controller::AppStatus::Streaming;
    state.running_tools = vec!["run_command".to_owned()];
    set_current_response(&mut state, "partial");

    // The animated label emits one span per character; the reduced-motion one
    // is a single flat span. Span shape, not color, is what proves the config
    // reached the render path (the sweep is time-driven and frozen in tests).
    let label_spans = |line: &ratatui::text::Line<'static>| {
        line.spans
            .iter()
            .filter(|span| span.content.chars().all(|ch| ch.is_alphabetic()))
            .map(|span| (span.content.to_string(), span.style.fg))
            .collect::<Vec<_>>()
    };
    let animated = activity_status_line(&render_snapshot(&state), false, ROOMY_ACTIVITY_WIDTH);
    state.config.reduced_motion = true;
    let still = activity_status_line(&render_snapshot(&state), false, ROOMY_ACTIVITY_WIDTH);

    assert_eq!(animated.to_string(), still.to_string());
    assert_eq!(label_spans(&animated).len(), "Working".len());
    let still_spans = label_spans(&still);
    assert_eq!(still_spans.len(), 1, "one flat label span: {still_spans:?}");
    assert_eq!(still_spans[0].0, "Working");
    assert!(
        still_spans
            .iter()
            .all(|(_, color)| *color == still_spans[0].1),
        "a reduced-motion label must not encode a gradient: {still_spans:?}"
    );
}

#[test]
fn transcript_cursor_returns_only_uncommitted_final_stream_tail() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("stable\n\n");

    assert_eq!(
        cursor.take_final_stream_remainder("stable\n\ntail"),
        Some("tail".to_owned())
    );
    assert_eq!(cursor.take_final_stream_remainder("stable\ntail"), None);
    assert!(!cursor.has_committed_stream());

    cursor.commit_stable_stream("stable\n\n");
    assert_eq!(cursor.take_final_stream_remainder("rewritten final"), None);
    assert!(!cursor.has_committed_stream());
}

#[test]
fn transcript_cursor_keeps_a_committed_prefix_when_the_stream_finalizes() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    let final_text = "Opening line\n\nFinal answer";
    let stable = cursor.pending_stable_source(final_text);
    cursor.commit_stable_stream(&stable);

    cursor.begin_stream("");

    assert_eq!(
        cursor.take_final_stream_remainder(final_text),
        Some("Final answer".to_owned())
    );
}

#[test]
fn transcript_cursor_reports_an_empty_tail_when_stream_rows_need_a_separator() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("table row\n\n");

    // The final history entry can contain exactly the rows already committed
    // during streaming. The draw loop uses this empty remainder as the handoff
    // point to insert one blank row before a follow-up user message.
    assert_eq!(
        cursor.take_final_stream_remainder("table row\n\n"),
        Some(String::new())
    );
}

#[test]
fn transcript_cursor_resets_when_a_new_stream_replaces_the_old_one() {
    let mut cursor = super::scrollback::TranscriptCursor::default();
    cursor.commit_stable_stream("first\n\n");
    cursor.begin_stream("second\n\ntail");

    assert_eq!(
        cursor.pending_stable_stream("second\n\ntail"),
        vec!["second"]
    );
}

/// A render-visible subagent row for picker / context-modal fixtures.
///
/// Tests seed the view directly: `SubagentController` mutates a live session,
/// which the render layer cannot name.
fn subagent_row(
    name: &str,
    task: &str,
    history: Vec<ChatMessage>,
    status: rustcode::controller::SubAgentStatus,
    active_turn: bool,
) -> rustcode::controller::SubAgentView {
    rustcode::controller::SubAgentView {
        id: 1,
        name: name.to_owned(),
        task: task.to_owned(),
        history: std::sync::Arc::new(history),
        status,
        active_turn,
        parent_id: None,
    }
}

#[test]
fn subagent_picker_renders_context_status_and_navigation_hint() {
    let mut state = RenderState::new();
    state.subagents.push(subagent_row(
        "agent-1",
        "inspect the parser",
        Vec::new(),
        rustcode::controller::SubAgentStatus::Running,
        true,
    ));
    state.show_subagent_picker = true;

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains("Agent contexts"));
    assert!(rendered.contains("agent-1"));
    assert!(rendered.contains("inspect the parser"));
    assert!(rendered.contains("main"));
}

#[test]
fn selected_subagent_renders_its_transcript_without_replacing_parent_history() {
    let mut state = RenderState::new();
    state.history.push(rustcode::controller::ChatMessage::new(
        "user",
        "parent task",
    ));
    let child = subagent_row(
        "agent-1",
        "child task",
        vec![rustcode::controller::ChatMessage::new(
            "assistant",
            "child result",
        )],
        rustcode::controller::SubAgentStatus::Running,
        true,
    );
    state.subagents.push(child.clone());
    state.selected_subagent = Some(child);
    state.selected_subagent_id = Some(1);

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains("agent-1"));
    assert!(rendered.contains("child result"));
    assert_eq!(state.history[0].content, "parent task");
}

#[test]
fn active_subagent_context_is_named_in_the_composer_footer() {
    let mut state = RenderState::new();
    let child = subagent_row(
        "agent-1",
        "child task",
        Vec::new(),
        rustcode::controller::SubAgentStatus::Running,
        true,
    );
    state.subagents.push(child.clone());
    state.selected_subagent = Some(child);
    state.selected_subagent_id = Some(1);

    let rendered = render_state_to_text(&mut state, 100, 30);

    assert!(rendered.contains(&format!("agent-1 · {}", state.model_name)));
}

#[test]
fn model_picker_open_then_close_leaves_no_duplicate_composer_or_stale_rows() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "test prompt"));
    state.config.models = vec![
        rustcode::controller::ModelProfile {
            name: "model-a".to_string(),
            url: "http://localhost/a".to_string(),
            model: "model-a".to_string(),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        },
        rustcode::controller::ModelProfile {
            name: "model-b".to_string(),
            url: "http://localhost/b".to_string(),
            model: "model-b".to_string(),
            context_window: None,
            engine: Some("Local".to_owned()),
            api_key: None,
            env_key: None,
            tool_protocol: None,
            enable_thinking: None,
            reasoning_effort: None,
            max_tokens: None,
            supports_vision: None,
            ..Default::default()
        },
    ];

    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    // Step 1: Open /model picker
    state.show_model_picker = true;
    let h1 = desired_height(&state, &mut transcript, 80, 24);
    assert!(
        h1 >= 14,
        "modal open should request at least 14 rows, got {h1}"
    );
    terminal
        .draw_height(h1, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    // Step 2: Select model and close picker
    state.show_model_picker = false;
    let h2 = desired_height(&state, &mut transcript, 80, 24);
    assert!(
        h2 == h1,
        "viewport should stay anchored on modal close, h1={h1}, h2={h2}"
    );
    terminal
        .draw_height(h2, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rendered = (0..24)
        .map(|r| (0..80).map(|c| buffer[(c, r)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    let prompt_count = rendered
        .lines()
        .filter(|line| line.contains("Ask RustCode to do anything"))
        .count();
    assert_eq!(
        prompt_count, 1,
        "must have exactly 1 composer prompt, got {prompt_count}:\n{rendered}"
    );
    assert!(
        !rendered.contains("Select model"),
        "picker header must not remain after close"
    );
}

#[test]
fn viewport_expansion_followed_by_shrink_clears_stale_rows() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();

    // Draw tall viewport with some state
    terminal
        .draw_height(16, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    // Draw smaller viewport
    terminal
        .draw_height(6, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    assert_eq!(terminal.area().height, 6);
    let buffer = terminal.backend().buffer();
    // Verify rows below the 6th row are empty
    for row in 6..20 {
        let line: String = (0..60).map(|col| buffer[(col, row)].symbol()).collect();
        assert!(
            line.trim().is_empty(),
            "row {row} below active viewport must be cleared, found: {line:?}"
        );
    }
}

#[test]
fn multiline_input_indentation_aligns_continuation_lines() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let mut state = RenderState::new();
    state.input_buffer = "first line\nsecond line\nthird line".to_string();
    state.cursor_position = state.input_buffer.len();

    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();

    terminal
        .draw_height(10, |f| {
            render_with_transcript(f, &mut state, &mut transcript);
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut rendered_lines = Vec::new();
    for row in 0..10 {
        let line: String = (0..80).map(|col| buffer[(col, row)].symbol()).collect();
        if !line.trim().is_empty() {
            rendered_lines.push(line);
        }
    }

    // Check that the first line starts with "› first line" and second line starts with "  second line"
    let first = rendered_lines
        .iter()
        .find(|l| l.contains("first line"))
        .expect("first line rendered");
    let second = rendered_lines
        .iter()
        .find(|l| l.contains("second line"))
        .expect("second line rendered");
    let third = rendered_lines
        .iter()
        .find(|l| l.contains("third line"))
        .expect("third line rendered");

    assert!(
        first.contains("› first line"),
        "first line must start with prompt chevron: {first}"
    );
    assert!(
        second.contains("  second line"),
        "second line must have 2-space padding: {second}"
    );
    assert!(
        third.contains("  third line"),
        "third line must have 2-space padding: {third}"
    );
}

#[test]
fn count_input_lines_accounts_for_prompt_indent() {
    assert_eq!(super::count_input_lines("", 80), 1);
    assert_eq!(super::count_input_lines("hello", 80), 1);
    assert_eq!(super::count_input_lines("hello\nworld", 80), 2);
    assert_eq!(super::count_input_lines("line1\nline2\nline3", 80), 3);

    // With width 10, indent is 2, available is 8 chars per line
    // "12345678" takes 8 chars + 2 indent = 10 -> fits on line 1
    // Next char triggers wrap to line 2
    assert_eq!(super::count_input_lines("12345678", 10), 1);
    assert_eq!(super::count_input_lines("123456789", 10), 2);
    assert_eq!(
        super::count_input_lines("a\nb\nc\nd\ne\nf\ng\nh\ni\nj", 80),
        10
    );
}

#[test]
fn input_wraps_at_word_boundaries_before_splitting_long_words() {
    let styled_chars = "alpha beta toggling speed"
        .chars()
        .map(|character| (character, ratatui::style::Style::default()))
        .collect::<Vec<_>>();
    let (lines, cursor_x, cursor_y) = super::wrap_input_chars(
        &styled_chars,
        18,
        styled_chars.len(),
        ratatui::style::Style::default(),
    );
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();

    assert_eq!(rendered.len(), 2);
    assert!(
        !rendered[0].contains("toggling"),
        "word was split: {rendered:?}"
    );
    assert!(
        rendered[1].contains("toggling speed"),
        "rendered: {rendered:?}"
    );
    assert_eq!((cursor_x, cursor_y), (16, 1));

    let long_word = "abcdefghijklmnop";
    let long_word_chars = long_word
        .chars()
        .map(|character| (character, ratatui::style::Style::default()))
        .collect::<Vec<_>>();
    let (long_lines, _, _) = super::wrap_input_chars(
        &long_word_chars,
        10,
        long_word_chars.len(),
        ratatui::style::Style::default(),
    );
    assert_eq!(long_lines.len(), 2, "long words still need hard wrapping");
}

#[test]
fn live_streaming_thinking_block_uses_thought_duration_not_total_generation_time() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(555));
    state.current_thought_started_at =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(2300));
    state.current_thought_tokens = 106;
    set_current_response(&mut state, "<think>\nAnalyzing the project\n");

    let lines = super::render_live_tail(&state, 80, 24);
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();
    let thought_header = rendered
        .iter()
        .find(|l| l.contains("Thought"))
        .expect("Thought header must be rendered");

    assert!(
        thought_header.contains("Thought for 2.3s, 106 tokens"),
        "thought header must show live thought stats: {thought_header}"
    );
    assert!(
        !thought_header.contains("555s"),
        "thought header must not show total generation duration: {thought_header}"
    );
}

#[test]
fn live_streaming_completed_thought_preserves_duration_while_rest_of_response_streams() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.generation_start_time =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(555));
    state.current_thought_started_at = None;
    state.current_thought_time_ms = 43000;
    state.current_thought_tokens = 1400;
    set_current_response(
        &mut state,
        "<think>\nAnalyzing the project\n</think>\nHere is the rest of the stream",
    );

    let lines = super::render_live_tail(&state, 80, 24);
    let rendered = lines
        .iter()
        .map(ratatui::text::Line::to_string)
        .collect::<Vec<_>>();
    let thought_header = rendered
        .iter()
        .find(|l| l.contains("Thought"))
        .expect("Thought header must be rendered");

    assert!(
        thought_header.contains("Thought for 43s, 1.4k tokens"),
        "thought header must show completed thought stats: {thought_header}"
    );
    assert!(
        !thought_header.contains("555s"),
        "thought header must not show total generation duration: {thought_header}"
    );
}

#[test]
fn command_child_lines_wrap_with_indentation() {
    use rustcode::controller::{ChatMessage, ToolCallRef, ToolResultRecord, Verbosity};

    let mut state = RenderState::new();
    state.verbosity = Verbosity::High;
    let long_cmd = "curl -sS https://example.com/api/v1/organizations/test -H 'Authorization: Bearer test_token' --data '{\"field\":\"very long content here\"}'";
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "run_command".to_owned(),
            arguments: serde_json::json!({"command": long_cmd}).to_string(),
        }]),
    );
    state.history.push(
        ChatMessage::new("tool", "ok")
            .answering(Some("call-1".to_owned()))
            .with_tool_result(ToolResultRecord {
                tool_name: "run_command".to_owned(),
                arguments_hash: String::new(),
                success: true,
                exit_code: Some(0),
                changed_paths: Vec::new(),
                truncated: false,
                full_output_artifact: None,
                ..Default::default()
            }),
    );

    let rendered = super::render_committed_tool_result_group(&state, &[1], 40, false);
    // Long chained commands collapse to a bounded preview (Codex-style,
    // max 2 visual lines) instead of flooding scrollback.
    assert!(
        rendered.len() <= 3,
        "long command should collapse to a bounded preview: {rendered:?}"
    );
    assert!(rendered[0].to_string().starts_with("• Ran"));
    assert!(rendered[1].to_string().starts_with("  └ Bash"));
    assert!(
        rendered.iter().any(|line| line.to_string().contains('…')),
        "collapsed preview should carry an ellipsis: {rendered:?}"
    );
    // Continuation lines must have indentation ("    ")
    for line in &rendered[2..] {
        let text = line.to_string();
        assert!(
            text.starts_with("    ") || text.is_empty(),
            "wrapped line must be indented with 4 spaces: {text:?}"
        );
    }
}

#[test]
fn default_turn_separator_is_lighter_color() {
    let default_palette = super::theme::get_palette("default");
    assert_eq!(
        default_palette.turn_separator,
        ratatui::style::Color::Rgb(90, 112, 126)
    );
}

#[test]
fn acceptance_context_modal_renders_usage_and_breakdown() {
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("user", "Hello assistant"));
    state.history.push(ChatMessage::new(
        "assistant",
        "Hello! How can I help you today?",
    ));
    state.show_context_modal = true;

    let breakdown = modals::calculate_context_breakdown(&render_snapshot(&state));
    assert!(breakdown.user_tokens > 0);
    assert!(breakdown.assistant_tokens > 0);
    assert!(breakdown.prompt_headroom_tokens < breakdown.context_window);

    let rendered = render_context_modal_to_text(&state, 120, 24);
    assert!(rendered.contains("context usage"), "rendered: {rendered:?}");
    assert!(rendered.contains("Esc to close"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Saved history estimate"),
        "rendered: {rendered:?}"
    );
    assert!(rendered.contains("User messages"), "rendered: {rendered:?}");
    assert!(
        rendered.contains("Agent responses"),
        "rendered: {rendered:?}"
    );
    assert!(
        rendered.contains("Estimated headroom"),
        "rendered: {rendered:?}"
    );

    let lines = rendered.lines().collect::<Vec<_>>();
    let header_row = lines
        .iter()
        .position(|line| line.contains("context usage"))
        .expect("context header should be rendered");
    let summary_row = lines
        .iter()
        .position(|line| {
            line.contains(" · ") && (line.contains(" prompt") || line.contains(" estimate"))
        })
        .expect("context summary should be rendered");
    let first_grid_row = lines
        .iter()
        .position(|line| line.chars().take(60).collect::<String>().contains("● "))
        .expect("context grid should be rendered");
    let category_header_row = lines
        .iter()
        .position(|line| line.contains("Saved history estimate"))
        .expect("category header should be rendered");
    let headroom_row = lines
        .iter()
        .position(|line| line.contains("Estimated headroom"))
        .expect("headroom row should be rendered");
    assert!(header_row > 0);
    assert!(
        lines[header_row - 1].trim().is_empty(),
        "context modal should have top padding above the header: {rendered:?}"
    );
    assert_eq!(summary_row, header_row + 2);
    assert_eq!(first_grid_row, summary_row);
    assert_eq!(category_header_row, summary_row + 2);
    assert!(
        headroom_row < lines.len() - 1,
        "context stats should fit within the full-height view: {rendered:?}"
    );
}

/// A `/context` state with one category well over `OVER_THRESHOLD_PCT` of the
/// window, so the screenshot and the emphasis assertions both have something to
/// show. Uses a profile with an explicit window so the percentages are stable.
fn context_state_with_over_threshold_category() -> RenderState {
    let mut state = RenderState::new();
    state.model_name = "claude-opus-5".to_owned();
    let mut profile = rustcode::controller::ModelProfile::default();
    profile.name = state.model_name.clone();
    profile.model = state.model_name.clone();
    profile.url = state.api_base_url.clone();
    profile.context_window = Some(128_000);
    state.config.models.clear();
    state.config.models.push(profile.clone());
    state.active_model_profile = Some(profile);
    state.active_context_window = 128_000;
    state.history.push(ChatMessage::new(
        "user",
        "summarise the release notes for the parser",
    ));
    state.history.push(ChatMessage::new(
        "assistant",
        "Here is a walkthrough of the parser changes, grouped by subsystem.",
    ));
    // `token` encodes close to one token per token under cl100k_base, so this
    // lands comfortably above the 20% threshold for the tool-call category.
    state
        .history
        .push(ChatMessage::new("tool", "token ".repeat(60_000)));
    state.history.push(ChatMessage::new(
        "tool",
        "render_state_to_ansi buffer_to_html render_snapshot",
    ));
    state.show_context_modal = true;
    state
}

/// Every shipped theme, so a per-theme claim is backed by a rendered panel
/// rather than by reading palette tables.
const CONTEXT_SHOT_THEMES: [&str; 8] = [
    "default",
    "rain",
    "cozy-rain",
    "light",
    "nord",
    "dracula",
    "tokyo-night",
    "sky",
];

#[test]
fn context_panel_tells_filled_from_empty_blocks_by_colour_not_only_by_glyph() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let state = context_state_with_over_threshold_category();
    for theme in CONTEXT_SHOT_THEMES {
        crate::ui::theme::set_active_theme(theme);
        let rows = render_context_modal_to_buffer(&state, 120, 24);
        let panel = COLOR_PANEL();
        let mut filled = Vec::new();
        let mut empty = Vec::new();
        for row in &rows {
            for cell in row {
                match cell.symbol.as_str() {
                    "●" => filled.push(cell.fg),
                    "□" => empty.push(cell.fg),
                    _ => {}
                }
            }
        }
        assert!(
            filled.len() > 2 && empty.len() > 2,
            "theme {theme}: expected filled and empty blocks, got {filled:?} / {empty:?}"
        );
        for color in empty.iter().chain(filled.iter()) {
            let ratio = super::categorical::contrast(*color, panel);
            assert!(
                ratio >= super::categorical::MIN_PANEL_CONTRAST,
                "theme {theme}: block {color:?} is only {ratio:.2}:1 against panel {panel:?}"
            );
        }
        // #1515 reported the empty block as effectively invisible on light
        // palettes; it must now be colour-distinct from every filled block.
        for free in empty.iter() {
            for used in filled.iter() {
                assert_ne!(
                    free, used,
                    "theme {theme}: empty block {free:?} matches a filled block {used:?}"
                );
                assert!(
                    super::categorical::separation(*free, *used)
                        >= super::categorical::MIN_SEPARATION,
                    "theme {theme}: empty block {free:?} and filled {used:?} are too close"
                );
            }
        }
    }
    crate::ui::theme::set_active_theme("default");
}

#[test]
fn context_panel_emphasises_an_over_threshold_category_in_the_rendered_buffer() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let state = context_state_with_over_threshold_category();
    let breakdown = modals::calculate_context_breakdown(&render_snapshot(&state));
    let share = breakdown.tool_tokens as f64 / breakdown.context_window as f64 * 100.0;
    assert!(
        share >= super::modals::OVER_THRESHOLD_PCT,
        "fixture must exercise the over-threshold path, got {share:.1}%"
    );

    crate::ui::theme::set_active_theme("default");
    let rows = render_context_modal_to_buffer(&state, 120, 24);
    let tool_row = rows
        .iter()
        .find(|row| row.iter().any(|c| c.symbol == "T"))
        .expect("tool-call stats row");
    let value: String = tool_row
        .iter()
        .skip_while(|c| c.symbol != "T")
        .skip(1)
        .map(|c| c.symbol.as_str())
        .collect();
    assert!(
        value.contains("tokens"),
        "tool-call row should carry its value: {value:?}"
    );
    let emphasized: Vec<&RenderedCell> = tool_row
        .iter()
        .filter(|c| c.symbol != " " && c.fg == COLOR_TIP() && c.modifier.contains(Modifier::BOLD))
        .collect();
    assert!(
        emphasized.len() >= 3,
        "over-threshold value should be bold tip: {:?}",
        tool_row
            .iter()
            .map(|c| (c.symbol.as_str(), c.fg, c.modifier))
            .collect::<Vec<_>>()
    );

    let normal_row = rows
        .iter()
        .find(|row| {
            row.iter()
                .any(|c| c.symbol == "S" && row.iter().any(|d| d.symbol == "k"))
        })
        .expect("skills stats row");
    assert!(
        !normal_row
            .iter()
            .any(|c| c.fg == COLOR_TIP() && c.modifier.contains(Modifier::BOLD)),
        "an under-threshold category must not borrow the emphasis: {normal_row:?}"
    );
    crate::ui::theme::set_active_theme("default");
}

#[test]
fn context_panel_screenshots_are_written_when_requested() {
    // Colour changes cannot be reviewed from `Color` equality or the text
    // goldens in `fixtures/`, so this dumps the rendered buffer as HTML (plus a
    // truecolor ANSI copy) for conversion to PNG by
    // `scripts/context-panel-screenshots.sh`, which also runs this test.
    let Ok(dir) = std::env::var("RUSTCODE_CONTEXT_PANEL_SHOTS") else {
        return;
    };
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    let state = context_state_with_over_threshold_category();

    for theme in CONTEXT_SHOT_THEMES {
        crate::ui::theme::set_active_theme(theme);
        let rows = render_context_modal_to_buffer(&state, 120, 24);
        let panel = COLOR_PANEL();
        std::fs::write(
            dir.join(format!("context-panel-{theme}.html")),
            buffer_to_html(&rows, &format!("/context — {theme}"), panel),
        )
        .expect("write html");
        std::fs::write(
            dir.join(format!("context-panel-{theme}.ansi")),
            buffer_to_ansi(&rows, panel),
        )
        .expect("write ansi");
    }
    crate::ui::theme::set_active_theme("default");
}

#[test]
fn footer_and_context_modal_use_provider_prompt_usage_for_the_active_context() {
    let mut state = RenderState::new();
    let mut profile = rustcode::controller::ModelProfile::default();
    profile.name = state.model_name.clone();
    profile.model = state.model_name.clone();
    profile.url = state.api_base_url.clone();
    profile.context_window = Some(100_000);
    state.config.models.clear();
    state.config.models.push(profile.clone());
    state.active_model_profile = Some(profile);
    state.active_context_window = 100_000;
    state
        .history
        .push(ChatMessage::new("tool", "x".repeat(80_000)));
    state.current_token_usage = Some(rustcode::controller::TokenUsage {
        prompt_tokens: 4_000,
        completion_tokens: 500,
        total_tokens: 4_500,
        ..Default::default()
    });
    state.show_context_modal = true;

    let snapshot = render_snapshot(&state);
    let active_usage = super::context_usage::context_usage(&snapshot);
    assert_eq!(active_usage.used_tokens, 4_000);
    assert_eq!(
        rustcode_core::status::context_remaining_percent(active_usage.used_tokens, 100_000),
        96
    );

    let breakdown = modals::calculate_context_breakdown(&snapshot);
    assert_eq!(
        breakdown.prompt_headroom_tokens, 96_000,
        "live prompt headroom must be based on provider prompt usage, not the stored transcript estimate"
    );
    let stored_history_estimate = breakdown
        .user_tokens
        .saturating_add(breakdown.assistant_tokens)
        .saturating_add(breakdown.tool_tokens)
        .saturating_add(breakdown.system_prompt_tokens)
        .saturating_add(breakdown.system_tools_tokens)
        .saturating_add(breakdown.skills_tokens)
        .saturating_add(breakdown.subagent_tokens);
    assert!(stored_history_estimate > active_usage.used_tokens as usize);
    let rendered = render_context_modal_to_text(&state, 120, 24);
    assert!(
        rendered.contains("4.0k/100.0k (4.0%) prompt"),
        "context summary must match provider prompt usage: {rendered:?}"
    );
    assert!(
        rendered.contains("Saved history estimate"),
        "estimated saved history must be distinguished from active prompt usage: {rendered:?}"
    );

    state.show_context_modal = false;
    let footer = render_state_to_text(&mut state, 120, 24);
    assert!(footer.contains("96% context left"), "footer: {footer:?}");
}

#[test]
fn selected_subagent_context_usage_and_categories_use_child_history() {
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new(
        "assistant",
        "parent history that is not active",
    ));
    let child_history = vec![ChatMessage::new("user", "child task")];
    let child = rustcode::controller::SubAgentView {
        id: 7,
        name: "reviewer".to_owned(),
        task: "review".to_owned(),
        history: std::sync::Arc::new(child_history.clone()),
        status: rustcode::controller::SubAgentStatus::Completed,
        active_turn: false,
        parent_id: None,
    };
    state.selected_subagent = Some(child.clone());
    state.subagents.push(child);
    state.selected_subagent_id = Some(7);
    state.show_context_modal = true;

    let snapshot = render_snapshot(&state);
    let breakdown = modals::calculate_context_breakdown(&snapshot);
    assert_eq!(
        breakdown.user_tokens,
        rustcode::controller::estimate_tokens("child task")
    );
    assert_eq!(breakdown.assistant_tokens, 0);
    assert_eq!(breakdown.subagent_tokens, 0);
}

/// A long assistant transcript plus one collapsed generic tool entry, which is
/// the shape every scroll/expand test below needs: enough committed rows to
/// scroll into, and one body long enough to need a cap.
fn state_with_a_scrollable_transcript() -> RenderState {
    use rustcode::controller::{ToolCallRef, ToolResultRecord};
    let mut state = RenderState::new();
    for index in 0..24 {
        state
            .history
            .push(ChatMessage::new("user", format!("reading item {index:02}")));
    }
    state.history.push(
        ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
            id: "call-1".to_owned(),
            name: "get_time".to_owned(),
            arguments: "{}".to_owned(),
        }]),
    );
    state.history.push(
        ChatMessage::new(
            "tool",
            format!(
                "get_time: {}",
                (0..400)
                    .map(|index| format!("row {index:03}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .answering(Some("call-1".to_owned()))
        .with_tool_result(ToolResultRecord {
            tool_name: "get_time".to_owned(),
            success: true,
            ..Default::default()
        }),
    );
    state
}

/// #1595: scrolling up during a stream holds the reading position, announces
/// the arriving output instead of jumping to it, and re-follows on request.
#[test]
fn streaming_while_scrolled_up_holds_the_rows_and_offers_a_return_to_latest() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = state_with_a_scrollable_transcript();
    let mut transcript = TranscriptState::default();

    // A frame with the transcript at the bottom paints no affordance at all.
    let (idle, input_area) =
        render_state_to_text_with_transcript_and_composer_area(&mut state, &mut transcript, 80, 20);
    assert!(
        !idle.contains("Back to bottom") && !idle.contains("Bottom"),
        "following paints no return affordance: {idle}"
    );
    assert!(transcript.is_following());

    transcript.scroll_up(4);
    let reading = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(
        reading.contains("Back to bottom"),
        "scrolling away offers a way back: {reading}"
    );
    let control = transcript
        .follow_control()
        .area()
        .expect("a painted control owns a rectangle");

    // The stream keeps arriving below the reader.
    set_current_response(&mut state, "streaming answer in progress");
    let still_reading = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert_eq!(
        rows_above_composer(&still_reading, input_area),
        rows_above_composer(&reading, input_area),
        "incoming output must not move the reading position"
    );
    assert!(!still_reading.contains("streaming answer in progress"));
    assert!(
        transcript.unseen_activity(),
        "output that arrived out of view is announced"
    );

    // The affordance stays in the dedicated row above the composer and says
    // so in the "new activity" wording.
    assert_eq!(
        control.bottom(),
        input_area.y,
        "the control sits directly above the composer"
    );
    let announced = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(
        announced.contains("New activity"),
        "unseen activity is worded as such: {announced}"
    );

    // Returning to the latest row re-follows and shows the new content.
    transcript.jump_to_latest();
    let following = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(transcript.is_following());
    assert!(!transcript.unseen_activity());
    assert!(!following.contains("Back to bottom"), "{following}");
    assert!(
        following.contains("streaming answer in progress"),
        "{following}"
    );
}

fn rows_above_composer(rendered: &str, input_area: ratatui::layout::Rect) -> &str {
    let mut lines = rendered.lines();
    for _ in 0..input_area.y {
        lines.next();
    }
    ""
}

/// #1595: the affordance never intercepts a pointer when it is hidden, and
/// degrades to shorter labels instead of overflowing a narrow terminal.
#[test]
fn the_return_to_latest_control_is_hidden_at_the_tail_and_degrades_when_narrow() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = state_with_a_scrollable_transcript();
    let mut transcript = TranscriptState::default();

    let _ = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert_eq!(
        transcript.follow_control().area(),
        None,
        "a hidden control must own no rectangle, or it would swallow clicks"
    );

    transcript.scroll_up(4);
    let wide = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(wide.contains("↓ Back to bottom · esc"), "{wide}");

    let narrow = render_state_to_text_with_transcript(&mut state, &mut transcript, 20, 20);
    assert!(
        !narrow.contains("esc"),
        "a narrow row drops the hint: {narrow}"
    );
    assert!(narrow.contains("↓"), "{narrow}");
    let control = transcript
        .follow_control()
        .area()
        .expect("the narrow label still paints");
    assert!(control.width <= 20, "{control:?}");
}

#[test]
fn the_return_to_latest_control_stays_above_the_input_and_hides_for_panels() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = state_with_a_scrollable_transcript();
    let mut transcript = TranscriptState::default();
    let _ = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    transcript.scroll_up(4);

    let (_, input_area) =
        render_state_to_text_with_transcript_and_composer_area(&mut state, &mut transcript, 80, 20);
    let control = transcript
        .follow_control()
        .area()
        .expect("the scrolled transcript shows a return control");
    assert_eq!(control.bottom(), input_area.y);

    state.show_session_modal = true;
    let panel = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(panel.contains("Session"), "{panel}");
    assert_eq!(
        transcript.follow_control().area(),
        None,
        "the panel owns the area and the hidden control owns no pointer target"
    );
}

/// #1593: an expanded body is bounded, and collapsing it restores exactly the
/// collapsed rows. Nothing is appended anywhere, so there is nothing to orphan.
#[test]
fn expanded_bodies_are_bounded_and_collapsing_restores_the_collapsed_rows() {
    use rustcode::controller::{ExpandOutcome, Verbosity};
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = state_with_a_scrollable_transcript();
    state.verbosity = Verbosity::Low;
    // Only commands and edit diffs expose expandable tool bodies.
    let call = &mut state.history[24].tool_calls[0];
    call.name = "run_command".to_owned();
    call.arguments = r#"{"command":"seq 0 399"}"#.to_owned();
    state.history[25].content = state.history[25]
        .content
        .replacen("get_time:", "run_command:", 1);
    state.history[25].tool_result.as_mut().unwrap().tool_name = "run_command".to_owned();
    let mut transcript = TranscriptState::default();

    let collapsed = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 30);
    assert!(collapsed.contains("(ctrl+o all"), "{collapsed}");
    assert!(
        !collapsed.contains("row 300"),
        "the collapsed window hides the body: {collapsed}"
    );

    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(candidates, [25], "the long command body is the candidate");
    let mut expanded = std::collections::HashSet::new();
    let (outcome, _) = rustcode::controller::toggle_all_expanded_bodies(&mut expanded, &candidates);
    assert_eq!(outcome, ExpandOutcome::ExpandedAll { count: 1 });
    state.expanded_thoughts = expanded;

    let expanded = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 30);
    assert!(
        !expanded.contains("(ctrl+o all"),
        "an expanded row drops its hint: {expanded}"
    );
    assert!(
        expanded.lines().count() <= 30,
        "the viewport, not the body, decides how much is visible"
    );

    // The cap is a property of the block, not of the viewport: even scrolled to
    // the very top of the expansion the body cannot claim unbounded rows, and
    // it says how many it dropped instead of pretending to be whole (#1593).
    let block = super::render_committed_tool_result_group(&state, &[25], 80, false)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    assert!(
        block.len() <= super::tool_result::TOOL_RESULT_TRANSCRIPT_MAX_LINES + 8,
        "an expanded body stays bounded: {} rows",
        block.len()
    );
    assert!(
        block
            .iter()
            .any(|line| line.contains("more lines · full output in the session log")),
        "a body past the cap says so: {block:?}"
    );

    // Collapsing again paints the collapsed rows and nothing else: the frame is
    // rebuilt from the snapshot, so an expanded body can never leave a residue
    // behind the way an append-only scrollback could.
    let (outcome, _) =
        rustcode::controller::toggle_all_expanded_bodies(&mut state.expanded_thoughts, &candidates);
    assert_eq!(outcome, ExpandOutcome::CollapsedAll { count: 1 });
    assert!(state.expanded_thoughts.is_empty());
    let recollapsed = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 30);
    assert_eq!(recollapsed, collapsed, "collapse is exactly the inverse");
}

/// #1594: one press moves the whole transcript, and the readout says how much
/// of it is open.
#[test]
fn ctrl_o_moves_every_collapsed_body_and_the_readout_counts_them() {
    use rustcode::controller::{ExpandOutcome, ToolCallRef, ToolResultRecord, Verbosity};
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.verbosity = Verbosity::Low;
    let fixtures = [
        (
            "run_command",
            r#"{"command":"printf first"}"#,
            "run_command: exit code: 0\nstdout:\nfirst detail row",
            None,
        ),
        (
            "run_command",
            r#"{"command":"printf second"}"#,
            "run_command: exit code: 0\nstdout:\nsecond detail row",
            None,
        ),
        (
            "write_to_file",
            r#"{"path":"src/added.rs","content":"fn added() {\n}"}"#,
            "write_to_file: wrote 'src/added.rs' (1 lines, 14 bytes)",
            Some("src/added.rs"),
        ),
    ];
    for (index, (name, arguments, result, changed_path)) in fixtures.iter().enumerate() {
        let call = format!("call-{index}");
        state.history.push(
            ChatMessage::new("assistant", "").with_tool_calls(vec![ToolCallRef {
                id: call.clone(),
                name: (*name).to_owned(),
                arguments: (*arguments).to_owned(),
            }]),
        );
        let mut record = ToolResultRecord {
            tool_name: (*name).to_owned(),
            success: true,
            ..Default::default()
        };
        if let Some(path) = changed_path {
            record.changed_paths = vec![(*path).to_owned()];
        }
        state.history.push(
            ChatMessage::new("tool", *result)
                .answering(Some(call))
                .with_tool_result(record),
        );
    }
    let mut transcript = TranscriptState::default();

    let collapsed = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 24);
    assert!(!collapsed.contains("expanded ·"), "{collapsed}");

    let candidates = super::collapsible_tool_indices(&render_snapshot(&state), 80);
    assert_eq!(
        candidates,
        [1, 3, 5],
        "every command/edit body is a candidate"
    );
    let (outcome, notice) =
        rustcode::controller::toggle_all_expanded_bodies(&mut state.expanded_thoughts, &candidates);
    assert_eq!(outcome, ExpandOutcome::ExpandedAll { count: 3 });
    assert_eq!(notice, "Expanded all tool output");

    let expanded = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 24);
    assert!(
        expanded.contains("3/3 expanded · ctrl+o all · ctrl+shift+o step"),
        "the whole-transcript state is visible while reading: {expanded}"
    );
    assert!(!expanded.contains("(ctrl+o all"), "{expanded}");

    let (outcome, _) =
        rustcode::controller::toggle_all_expanded_bodies(&mut state.expanded_thoughts, &candidates);
    assert_eq!(outcome, ExpandOutcome::CollapsedAll { count: 3 });
    assert!(state.expanded_thoughts.is_empty());
    let recollapsed = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 24);
    assert!(!recollapsed.contains("expanded ·"), "{recollapsed}");
    assert!(!recollapsed.contains("(ctrl+o all"), "{recollapsed}");
}

/// #1594: a selection releases follow, so pointing at a row is enough to keep
/// the viewport still while the model streams.
#[test]
fn a_live_selection_also_releases_follow() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = state_with_a_scrollable_transcript();
    let mut transcript = TranscriptState::default();
    let _ = render_state_to_text_with_transcript(&mut state, &mut transcript, 80, 20);
    assert!(transcript.is_following());

    select_transcript_text(&mut state, &mut transcript, (2, 2), (20, 4));
    assert!(transcript.selection.is_active());
    assert!(!transcript.is_following());
}

#[test]
fn generated_recap_shows_next_action_and_stays_bounded_at_narrow_widths() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("assistant", r#"{"summary":"The fix is implemented and tested.","next_action":"Install it locally."}"#).as_conversation_recap());
    for width in [8, 16, 32, 80] {
        let rendered = super::render_committed_history_block(&state, 0, width);
        assert!(
            rendered.iter().all(|line| line.width() <= width as usize),
            "{rendered:?}"
        );
        let text = rendered.iter().map(Line::to_string).collect::<String>();
        if width >= 32 {
            assert!(text.contains("Next:"), "{text}");
        }
        assert!(!text.contains("summary"));
        assert!(
            rendered
                .iter()
                .flat_map(|line| &line.spans)
                .any(|span| span.content.contains("N")
                    && span.style.add_modifier.contains(Modifier::BOLD))
                || width < 10
        );
    }
}

#[test]
fn footer_shows_the_model_without_a_running_indicator() {
    // The running indicator moved to the bottom of the chat, so the footer is
    // model + workspace only — no status words, spinner, or queue markers.
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    for status in [
        AppStatus::Idle,
        AppStatus::Streaming,
        AppStatus::Queued,
        AppStatus::AwaitingQuestion,
        AppStatus::AwaitingToolConfirmation,
    ] {
        let mut state = RenderState::new();
        state.status = status.clone();
        state.config.reduced_motion = true;
        let snapshot = render_snapshot(&state);
        let model = snapshot.model_name().to_string();
        let mut terminal =
            crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(160, 1))
                .unwrap();
        terminal
            .draw(|frame| {
                super::render_composer_footer(frame, frame.area(), &snapshot, None, false);
            })
            .unwrap();
        let row = (0..160)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect::<String>();
        assert!(
            row.contains(&model),
            "{status:?} must keep the model: {row}"
        );
        for unwanted in ["•", "◦", "⠋", "⠙", "⠹", "Working", "Queued", "Waiting"] {
            assert!(
                !row.contains(unwanted),
                "{status:?} leaked {unwanted}: {row}"
            );
        }
        assert!(
            !row.contains("esc interrupt") && !row.contains("Thinking"),
            "{row}"
        );
    }
}

#[test]
fn running_turn_shows_a_plain_spinner_and_model_at_the_bottom_of_the_chat() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    for status in [AppStatus::Streaming, AppStatus::Queued] {
        let mut state = RenderState::new();
        state.status = status.clone();
        state.config.reduced_motion = true;
        state.history.push(ChatMessage::new("user", "hello"));
        let snapshot = render_snapshot(&state);
        let model = snapshot.model_name().to_string();
        let mut transcript = TranscriptState::default();
        let mut terminal =
            crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(80, 12))
                .unwrap();
        terminal
            .draw(|frame| {
                let snapshot = render_snapshot(&state);
                let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
            })
            .unwrap();
        let rows = (0..12)
            .map(|y| {
                (0..80)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let indicator = rows
            .iter()
            .find(|row| row.contains(&model))
            .unwrap_or_else(|| panic!("{status:?} must show the model in the chat: {rows:?}"));
        assert!(
            indicator.trim_start().starts_with('•'),
            "{status:?} indicator must lead with the spinner: {indicator:?}"
        );
        // Deliberately plain: no status words or elapsed clocks.
        for unwanted in ["Working", "Queued", "Waiting", "Tokens/s"] {
            assert!(
                !indicator.contains(unwanted),
                "{status:?} indicator leaked {unwanted}: {indicator:?}"
            );
        }
    }
}

#[test]
fn the_running_indicator_sits_directly_under_the_transcript() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.config.reduced_motion = true;
    // One short message in a tall viewport: plenty of blank rows below it.
    state.history.push(ChatMessage::new("user", "hello"));
    let snapshot = render_snapshot(&state);
    let model = snapshot.model_name().to_string();

    let height = 24u16;
    let mut transcript = TranscriptState::default();
    let mut terminal =
        crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(80, height))
            .unwrap();
    terminal
        .draw(|frame| {
            let snapshot = render_snapshot(&state);
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();

    let rows = (0..height)
        .map(|y| {
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    let prompt = rows
        .iter()
        .position(|row| row.contains("hello"))
        .expect("the user message is on screen");
    // The indicator is the row that leads with the spinner and names the model.
    // The welcome panel and the composer footer also name the model, so anchor
    // on the glyph.
    let indicator = rows
        .iter()
        .position(|row| row.trim_start().starts_with('•') && row.contains(&model))
        .unwrap_or_else(|| panic!("the indicator must be on screen: {rows:?}"));

    // Preserve the user panel bottom padding and leave a blank row above the indicator.
    assert_eq!(
        terminal.backend().buffer()[(0, (prompt + 1) as u16)].bg,
        COLOR_PANEL()
    );
    assert_eq!(
        terminal.backend().buffer()[(0, (prompt + 2) as u16)].bg,
        COLOR_BG()
    );
    assert_eq!(
        indicator,
        prompt + 3,
        "the indicator must sit under the transcript, not at the viewport bottom: {rows:?}"
    );
    assert!(
        indicator + 2 < height as usize,
        "the chat had spare room, so a bottom-anchored indicator was not expected: {rows:?}"
    );
}

#[test]
fn idle_chat_shows_no_running_indicator() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.config.reduced_motion = true;
    state.history.push(ChatMessage::new("user", "hello"));
    state
        .history
        .push(ChatMessage::new("assistant", "done already"));
    let snapshot = render_snapshot(&state);
    let model = snapshot.model_name().to_string();
    let mut transcript = TranscriptState::default();
    let mut terminal =
        crate::inline_terminal::InlineTerminal::new(ratatui::backend::TestBackend::new(80, 12))
            .unwrap();
    terminal
        .draw(|frame| {
            let snapshot = render_snapshot(&state);
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();
    let chat_rows = (0..9)
        .map(|y| {
            (0..80)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(
        !chat_rows.iter().any(|row| row.contains(&model)),
        "idle chat must not show a running indicator: {chat_rows:?}"
    );
}

#[test]
fn reduced_motion_chat_indicator_is_static_and_live_tools_are_hidden() {
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.config.reduced_motion = true;
    state.history.push(ChatMessage::new("user", "hello"));
    set_current_response(&mut state, "visible assistant text");
    std::sync::Arc::make_mut(&mut state.live_tool_calls).push(
        rustcode::controller::LiveToolCall::new(
            "call-1",
            None,
            "run_command",
            "Bash",
            "secret command",
        ),
    );
    let text = render_state_to_text(&mut state, 100, 20);
    assert!(text.contains("visible assistant text"));
    // A full chat needs no indicator row: the streaming text is the signal.
    // (Dedicated tests cover the indicator in a chat with spare room.)
    assert!(!text.contains("Working"));
    assert!(!text.contains("secret command"));
    assert!(!text.contains("Running"));
    assert!(!text.contains("Queued"));
}

#[test]
fn reduced_motion_renders_the_chat_indicator_as_a_static_bullet() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.status = AppStatus::Streaming;
    state.config.reduced_motion = true;
    let snapshot = render_snapshot(&state);
    let indicator =
        super::live_running_indicator(&snapshot).expect("a running turn has an indicator");
    let text = indicator
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(text, format!("• {}", snapshot.model_name()), "{text}");
    assert!(
        !text.contains('⠋'),
        "reduced motion must not animate: {text}"
    );
}

#[test]
fn slash_popup_is_flush_with_composer_when_tools_are_expanded() {
    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state
        .history
        .push(ChatMessage::new("tool", "get_time: detail"));
    state.expanded_thoughts.insert(0);
    state.input_buffer = "/verbosity".to_owned();
    state.cursor_position = state.input_buffer.len();
    state.active_suggestion_index = Some(0);
    let rendered = render_state_to_text(&mut state, 80, 24);
    let rows = rendered.lines().collect::<Vec<_>>();
    assert!(!rendered.contains("expanded ·"), "{rendered}");
    let input = rows
        .iter()
        .rposition(|line| line.contains("› /verbosity"))
        .unwrap();
    let popup = rows
        .iter()
        .position(|line| line.contains("/verbosity"))
        .unwrap();
    assert_eq!(
        input - popup,
        2,
        "only composer top padding belongs between picker and input: {rendered}"
    );
}

#[test]
fn streaming_timer_only_tick_keeps_frame_stable() {
    use crate::inline_terminal::InlineTerminal as Terminal;
    use ratatui::backend::TestBackend;

    let _theme_guard = THEME_TEST_LOCK.lock().expect("theme test lock");
    let mut state = RenderState::new();
    state.history.push(ChatMessage::new("user", "hello"));
    state.status = AppStatus::Streaming;
    set_current_response(&mut state, "streamed output");
    // Reuse one TranscriptState across ticks so committed caches stay warm,
    // exactly as the runtime does between 16ms frames.
    let mut transcript = TranscriptState::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            let snapshot = render_snapshot(&state);
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();
    let before = terminal.backend().buffer().clone();
    let scroll_before = transcript.scroll_rows();
    // Identical content on the next tick (only wall-clock advanced in prod;
    // shimmer/spinner are frozen under cfg(test)) must repaint identically
    // with no scroll jitter.
    terminal
        .draw(|frame| {
            let snapshot = render_snapshot(&state);
            let _ = render_with_transcript_snapshot(frame, &snapshot, &mut transcript);
        })
        .unwrap();
    assert_eq!(before, *terminal.backend().buffer());
    assert_eq!(scroll_before, transcript.scroll_rows());
}
