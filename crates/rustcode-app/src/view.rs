use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use gpui_kit::{
    Anchor, Animation, AnimationExt, Context, FocusHandle, Focusable, KeyDownEvent,
    PathPromptOptions, Render, Window, actions,
    component::{
        Disableable, Icon, IconName, Root, Selectable, Sizable, StyledExt, TITLE_BAR_HEIGHT, Theme,
        TitleBar,
        button::{Button, ButtonVariants},
        dialog::{AlertDialog, DialogButtonProps},
        input::{Enter, Input, InputEvent, InputState, Position, Textarea, TextareaState},
        menu::{DropdownMenu, PopupMenuItem},
        message_scroller::{MessageScroller, MessageScrollerState},
        scroll::ScrollableElement,
        sidebar::{Sidebar, SidebarCollapsible, SidebarGroup, SidebarMenu, SidebarMenuItem},
        text::{TextView, TextViewStyle},
        tooltip::Tooltip,
    },
    div,
    prelude::*,
    px, rgb,
};
use rustcode::controller::{
    ApprovalChoice, Command, ControllerEvent, ControllerSnapshot, ControllerUpdate, SessionChoice,
};

use super::{CloseWindow, MinimizeWindow};
actions!(
    rustcode_app,
    [
        ToggleSidebar,
        OpenSettings,
        ToggleChatSearch,
        CloseChatSearch
    ]
);

const SIDEBAR_WIDTH: f32 = 270.;
const MAIN_PANE_INSET: f32 = 24.;
#[cfg(target_os = "macos")]
const TITLE_BAR_LEFT_PADDING: f32 = 80.;
#[cfg(not(target_os = "macos"))]
const TITLE_BAR_LEFT_PADDING: f32 = 12.;
const SIDEBAR_TOGGLE_SIZE: f32 = 24.;
const TITLE_BAR_CHILD_GAP: f32 = 8.;
const SETTINGS_RAIL_WIDTH: f32 = 180.;
const SETTINGS_CONTENT_WIDTH: f32 = 680.;
const MESSAGE_COPY_CONTROL_HEIGHT: f32 = 24.;
const MESSAGE_COPY_CONTROL_BOTTOM_OFFSET: f32 = 0.;
const SIDEBAR_TITLE_MARGIN: f32 = SIDEBAR_WIDTH + MAIN_PANE_INSET
    - TITLE_BAR_LEFT_PADDING
    - SIDEBAR_TOGGLE_SIZE
    - TITLE_BAR_CHILD_GAP;

fn slash_menu_key_decision(
    draft: &str,
    selected: usize,
    dismissed: bool,
    key: &str,
    composer_focused: bool,
) -> crate::slash::SlashInteraction {
    if composer_focused {
        crate::slash::slash_interaction(draft, selected, dismissed, key)
    } else {
        crate::slash::SlashInteraction::Ignore
    }
}

fn current_branch(project: &Path) -> Option<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(project)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    (!branch.is_empty()).then(|| branch.to_owned())
}

fn copy_control_stays_in_hover_region(bottom_offset: f32, reserved_height: f32) -> bool {
    bottom_offset >= 0. && reserved_height >= MESSAGE_COPY_CONTROL_HEIGHT
}

fn approval_in_flight_after_snapshot(
    current_request_id: Option<String>,
    pending_request_id: Option<&str>,
) -> Option<String> {
    current_request_id.filter(|request_id| Some(request_id.as_str()) == pending_request_id)
}

use crate::search::ConversationSearch;
use crate::theme::NativePalette as Palette;
use crate::{
    backend::{
        NativeBackend, project_selection_command, resolve_resume_workspace, resume_session_command,
    },
    projection::{
        ChatViewState, ComposerAction, ProjectionRow, ToolStatus, can_submit, project_rows,
        toggle_option, tool_row_presentation,
    },
};

pub struct AppView {
    backend: NativeBackend,
    focus_handle: FocusHandle,
    launch_dir: PathBuf,
    selected_project: PathBuf,
    composer: gpui_kit::Entity<TextareaState>,
    _composer_subscription: gpui_kit::Subscription,
    pending_images: Vec<crate::image_attachment::ImageAttachment>,
    question_answer: gpui_kit::Entity<TextareaState>,
    search_input: gpui_kit::Entity<InputState>,
    _search_subscription: gpui_kit::Subscription,
    messages: gpui_kit::Entity<MessageScrollerState>,
    navigation: AppNavigation,
    recent_sessions: Vec<SessionChoice>,
    chat_state: ChatViewState,
    selected_question_options: Vec<String>,
    status: Option<String>,
    pending_prompt: Option<String>,
    pending_model_selection: Option<String>,
    starting_new_session: bool,
    clear_composer_on_render: bool,
    slash_selection: usize,
    slash_picker_dismissed: bool,
    sidebar_collapsed: bool,
    git_branch: Option<String>,
    expanded_thoughts: HashSet<(usize, usize)>,
    expanded_tools: HashSet<(usize, usize)>,
    turn_timer_epoch: u64,
    chat_search_open: bool,
    conversation_search: ConversationSearch,
    reset_search_input_on_render: bool,
    focus_search_on_render: bool,
    focus_composer_on_render: bool,
    approval_in_flight: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppDestination {
    #[default]
    Chat,
    Settings(SettingsSection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSection {
    General,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AppNavigation {
    destination: AppDestination,
    chat_snapshot: Option<ControllerSnapshot>,
}

impl AppNavigation {
    #[cfg(test)]
    fn new(chat_snapshot: Option<ControllerSnapshot>) -> Self {
        Self {
            destination: AppDestination::Chat,
            chat_snapshot,
        }
    }

    fn open_settings(&mut self) {
        self.destination = AppDestination::Settings(SettingsSection::General);
    }

    fn open_chat(&mut self) {
        self.destination = AppDestination::Chat;
    }
}

impl AppView {
    pub fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.navigation.open_settings();
        cx.notify();
    }

    pub fn new(
        backend: NativeBackend,
        launch_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let status = backend
            .controller()
            .send(Command::ListSessions)
            .err()
            .map(|error| format!("Controller error: {error:?}"));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask RustCode anything")
                .auto_grow(1, 4)
                .submit_on_enter(true)
        });
        let composer_subscription = cx.subscribe(&composer, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.slash_selection = 0;
                this.slash_picker_dismissed = false;
                cx.notify();
            }
        });
        let question_answer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Or type your answer")
                .auto_grow(1, 3)
        });
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search conversation"));
        let search_view = cx.entity().downgrade();
        let search_subscription =
            cx.subscribe(&search_input, move |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = input.read(cx).value().to_string();
                    let search_view = search_view.clone();
                    cx.defer(move |cx| {
                        let _ = search_view.update(cx, |view, cx| {
                            let rows = view.display_rows();
                            let rows = Self::searchable_rows(&rows);
                            view.conversation_search.set_query(query, &rows);
                            view.scroll_to_current_match(cx);
                            cx.notify();
                        });
                    });
                }
            });
        let messages = cx.new(|cx| MessageScrollerState::new(0, cx));
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            backend,
            focus_handle,
            git_branch: current_branch(&launch_dir),
            expanded_thoughts: HashSet::new(),
            expanded_tools: HashSet::new(),
            turn_timer_epoch: 0,
            selected_project: launch_dir.clone(),
            launch_dir,
            composer,
            _composer_subscription: composer_subscription,
            pending_images: Vec::new(),
            question_answer,
            search_input,
            _search_subscription: search_subscription,
            messages,
            navigation: AppNavigation::default(),
            recent_sessions: Vec::new(),
            chat_state: ChatViewState::default(),
            selected_question_options: Vec::new(),
            status,
            pending_prompt: None,
            pending_model_selection: None,
            starting_new_session: false,
            clear_composer_on_render: false,
            slash_selection: 0,
            slash_picker_dismissed: false,
            sidebar_collapsed: false,
            chat_search_open: false,
            conversation_search: ConversationSearch::default(),
            reset_search_input_on_render: false,
            focus_search_on_render: false,
            focus_composer_on_render: false,
            approval_in_flight: None,
        }
    }

    pub fn take_updates(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<ControllerEvent> {
        self.backend.take_updates()
    }

    pub fn apply_event(&mut self, event: ControllerEvent, cx: &mut Context<Self>) {
        if self
            .navigation
            .chat_snapshot
            .as_ref()
            .is_some_and(|snapshot| event.generation < snapshot.generation)
        {
            return;
        }

        let starts_turn = matches!(
            &event.update,
            ControllerUpdate::Turn(rustcode::controller::TurnUpdate::PromptStarted(_))
        );

        if let ControllerUpdate::Turn(rustcode::controller::TurnUpdate::QuestionRequested(question)) =
            &event.update
            && self.chat_state.pending_question() != Some(question)
        {
            self.selected_question_options.clear();
        }
        if let ControllerUpdate::Turn(rustcode::controller::TurnUpdate::ApprovalRequested(
            approvals,
        )) = &event.update
            && self.approval_in_flight.as_ref().is_some_and(|request_id| {
                !approvals
                    .iter()
                    .any(|approval| &approval.request_id == request_id)
            })
        {
            self.approval_in_flight = None;
        }

        self.chat_state.apply_update(event.update.clone());
        match event.update {
            ControllerUpdate::Snapshot(snapshot) => {
                self.approval_in_flight = approval_in_flight_after_snapshot(
                    self.approval_in_flight.take(),
                    snapshot
                        .pending_approval
                        .as_ref()
                        .map(|approval| approval.request_id.as_str()),
                );
                if !snapshot.sessions.is_empty() {
                    self.recent_sessions = snapshot.sessions.clone();
                }
                let prior_session_id = self
                    .navigation
                    .chat_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.session_id.clone());
                let prior_generation = self
                    .navigation
                    .chat_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.generation);
                let session_id = snapshot.session_id.clone();
                let started_session = session_id.is_some()
                    && (session_id != prior_session_id
                        || prior_generation != Some(snapshot.generation));
                let previous_question = self
                    .navigation
                    .chat_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.pending_question.as_ref());
                let next_question = snapshot.pending_question.as_ref();
                if previous_question != next_question {
                    self.selected_question_options.clear();
                }
                self.navigation.chat_snapshot = Some(snapshot);
                self.status = None;
                if started_session {
                    self.expanded_thoughts.clear();
                    self.expanded_tools.clear();
                    self.chat_state.clear_turn_elapsed();
                    self.chat_search_open = false;
                    self.conversation_search.clear();
                    self.reset_search_input_on_render = true;
                    self.focus_composer_on_render = true;
                    self.starting_new_session = false;
                }
                if let Some(workspace) = self
                    .navigation
                    .chat_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.workspace.clone())
                {
                    self.selected_project = workspace;
                }
                self.git_branch = current_branch(&self.selected_project);
                if started_session {
                    if let Some(model_id) = self.pending_model_selection.take() {
                        self.send_command(Command::SelectModel(model_id), cx);
                    }
                    if let Some(prompt) = self.pending_prompt.take()
                        && self.send_command(Command::Submit(prompt), cx)
                    {
                        self.clear_composer_on_render = true;
                        self.pending_images.clear();
                    }
                }
            }
            ControllerUpdate::Turn(
                rustcode::controller::TurnUpdate::TurnFinished
                | rustcode::controller::TurnUpdate::Cancelled,
            ) => {
                if let Err(error) = self.backend.controller().send(Command::ListSessions) {
                    self.status = Some(format!("Could not refresh sessions: {error:?}"));
                }
            }
            ControllerUpdate::Turn(_) => {}
            ControllerUpdate::Error(_) => {
                self.status = None;
                self.approval_in_flight = None;
                if self.starting_new_session {
                    self.starting_new_session = false;
                    self.pending_prompt = None;
                }
            }
        }
        if starts_turn {
            self.turn_timer_epoch = self.turn_timer_epoch.wrapping_add(1);
            let epoch = self.turn_timer_epoch;
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    let active = this
                        .update(cx, |this, cx| {
                            if this.turn_timer_epoch == epoch && this.chat_state.turn_active() {
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !active {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub fn controller_stopped(&mut self, cx: &mut Context<Self>) {
        self.status = Some("Controller update stream closed".to_owned());
        cx.notify();
    }

    fn start_workspace(&mut self, workspace: PathBuf, cx: &mut Context<Self>) {
        if let Some(command) = project_selection_command(Some(workspace)) {
            if let Command::StartNew(path) = &command {
                self.selected_project = path.clone();
                self.git_branch = current_branch(path);
                self.starting_new_session = true;
            }
            if !self.send_command(command, cx) {
                self.starting_new_session = false;
                self.pending_prompt = None;
            }
        }
    }

    fn start_new_chat(&mut self, cx: &mut Context<Self>) {
        self.navigation.open_chat();
        let project = if self.selected_project.is_dir() {
            self.selected_project.clone()
        } else if self.launch_dir.is_dir() {
            self.launch_dir.clone()
        } else {
            self.choose_project_and_start(cx);
            return;
        };
        self.start_workspace(project, cx);
    }

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    pub(crate) fn toggle_chat_search(&mut self, cx: &mut Context<Self>) {
        if self.chat_search_open {
            self.close_chat_search(cx);
            return;
        }
        if self.search_input.read(cx).value().as_ref() != self.conversation_search.query() {
            self.reset_search_input_on_render = true;
            self.conversation_search.clear();
        }
        self.chat_search_open = true;
        self.focus_search_on_render = true;
        cx.notify();
    }

    pub(crate) fn close_chat_search(&mut self, cx: &mut Context<Self>) {
        if !self.chat_search_open {
            return;
        }
        self.chat_search_open = false;
        self.conversation_search.clear();
        self.reset_search_input_on_render = true;
        self.focus_composer_on_render = true;
        cx.notify();
    }

    fn navigate_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        if forward {
            self.conversation_search.next();
        } else {
            self.conversation_search.previous();
        }
        self.scroll_to_current_match(cx);
        cx.notify();
    }

    fn render_chat_search(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let result = match (
            self.conversation_search.current_number(),
            self.conversation_search.count(),
        ) {
            (Some(current), count) => format!("{current} of {count}"),
            (None, 0) if self.conversation_search.query().is_empty() => String::new(),
            _ => "No matches".to_owned(),
        };
        let previous_view = cx.entity().downgrade();
        let next_view = cx.entity().downgrade();
        let close_view = cx.entity().downgrade();
        div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_1()
            .border_color(rgb(Palette::BORDER_SUBTLE))
            .rounded_lg()
            .bg(rgb(Palette::SURFACE_ELEVATED))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .on_action(cx.listener(|this, action: &Enter, _, cx| {
                        this.navigate_search(!action.shift, cx);
                    }))
                    .child(
                        Input::new(&self.search_input)
                            .id("conversation-search-input")
                            .appearance(false)
                            .bordered(false)
                            .w_full(),
                    ),
            )
            .child(
                div()
                    .min_w(px(74.))
                    .text_xs()
                    .text_color(rgb(Palette::TEXT_SECONDARY))
                    .child(result),
            )
            .child(
                Button::new("search-previous")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronUp)
                    .accessibility_label("Previous match")
                    .tooltip("Previous match (Shift+Enter)")
                    .on_click(move |_, _, cx| {
                        let _ =
                            previous_view.update(cx, |this, cx| this.navigate_search(false, cx));
                    }),
            )
            .child(
                Button::new("search-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronDown)
                    .accessibility_label("Next match")
                    .tooltip("Next match (Enter)")
                    .on_click(move |_, _, cx| {
                        let _ = next_view.update(cx, |this, cx| this.navigate_search(true, cx));
                    }),
            )
            .child(
                Button::new("search-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .accessibility_label("Close search")
                    .tooltip("Close search (Esc)")
                    .on_click(move |_, _, cx| {
                        let _ = close_view.update(cx, |this, cx| this.close_chat_search(cx));
                    }),
            )
    }

    fn choose_project_and_start(&mut self, cx: &mut Context<Self>) {
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose workspace".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = selected.await;
            let _ = this.update(&mut *cx, |this, cx| match result {
                Ok(Ok(Some(paths))) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.start_workspace(path, cx);
                    }
                }
                Ok(Ok(None)) => {
                    this.pending_prompt = None;
                }
                Ok(Err(error)) => {
                    this.pending_prompt = None;
                    this.status = Some(format!("Folder picker error: {error}"));
                    cx.notify();
                }
                Err(error) => {
                    this.pending_prompt = None;
                    this.status = Some(format!("Folder picker error: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn resume_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        self.navigation.open_chat();
        // Never resume in a stale directory: prefer the selected project,
        // fall back to the launch directory, and offer the folder picker
        // when neither is valid (issue #1377).
        let Some(workspace) = resolve_resume_workspace(&self.selected_project, &self.launch_dir)
        else {
            self.choose_project_and_resume(session_id, cx);
            return;
        };
        let command = resume_session_command(session_id, workspace.clone());
        self.selected_project = workspace;
        self.send_command(command, cx);
    }

    fn choose_project_and_resume(&mut self, session_id: String, cx: &mut Context<Self>) {
        let selected = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose project folder to resume in".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = selected.await;
            let _ = this.update(&mut *cx, |this, cx| match result {
                Ok(Ok(Some(paths))) => {
                    if let Some(workspace) = paths.into_iter().next() {
                        this.selected_project = workspace.clone();
                        this.send_command(
                            resume_session_command(session_id.clone(), workspace),
                            cx,
                        );
                    }
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    this.status = Some(format!("Folder picker error: {error}"));
                    cx.notify();
                }
                Err(error) => {
                    this.status = Some(format!("Folder picker error: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.pending_prompt.is_some()
            || self.starting_new_session
            || !self.chat_state.composer_enabled()
        {
            return false;
        }
        let draft = self.composer.read(cx).value().to_string();
        if let crate::slash::SlashInteraction::Complete {
            value,
            cursor_offset,
        } = crate::slash::slash_interaction(
            &draft,
            self.slash_selection,
            self.slash_picker_dismissed,
            "Enter",
        ) {
            self.apply_slash_completion(value, cursor_offset, window, cx);
            return true;
        }
        let answering_question = self.chat_state.pending_question().or_else(|| {
            self.navigation
                .chat_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_question.as_ref())
        });
        let is_answering_question = answering_question.is_some();
        let text = if is_answering_question {
            draft
        } else {
            crate::image_attachment::prompt_with_images(&draft, &self.pending_images)
        };
        if !can_submit(&text) {
            return false;
        }
        let command = if let Some(question) = answering_question {
            crate::projection::answer_for_question(question, None, &text)
                .map(Command::AnswerQuestion)
        } else {
            if self
                .navigation
                .chat_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.session_id.is_some())
            {
                Some(Command::Submit(text))
            } else {
                if self.selected_project.is_dir() {
                    self.pending_prompt = Some(text);
                    self.start_workspace(self.selected_project.clone(), cx);
                } else if self.launch_dir.is_dir() {
                    self.selected_project = self.launch_dir.clone();
                    self.pending_prompt = Some(text);
                    self.start_workspace(self.launch_dir.clone(), cx);
                } else {
                    self.pending_prompt = Some(text);
                    self.choose_project_and_start(cx);
                }
                None
            }
        };
        if let Some(command) = command
            && self.send_command(command, cx)
        {
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            if !is_answering_question {
                self.pending_images.clear();
            }
            cx.notify();
        }
        false
    }

    fn apply_slash_completion(
        &mut self,
        value: String,
        cursor_offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |state, cx| {
            state.set_value(&value, window, cx);
            state.set_cursor_position(Position::new(0, cursor_offset as u32), window, cx);
            state.focus(window, cx);
        });
        self.slash_picker_dismissed = true;
        cx.notify();
    }

    fn send_command(&mut self, command: Command, cx: &mut Context<Self>) -> bool {
        self.chat_state.begin_user_action();
        if let Err(error) = self.backend.controller().send(command) {
            let message = format!("Controller error: {error:?}");
            self.chat_state.set_error(message);
            self.status = None;
            cx.notify();
            false
        } else {
            true
        }
    }

    fn display_rows(&self) -> Vec<DisplayRow> {
        let mut rows = self
            .navigation
            .chat_snapshot
            .as_ref()
            .map(|snapshot| project_rows(&snapshot.transcript, &snapshot.live_response))
            .unwrap_or_default();
        if !self.chat_state.stream_rows().is_empty() {
            for streamed in self.chat_state.stream_rows_with_elapsed() {
                match (rows.last_mut(), &streamed) {
                    (
                        Some(ProjectionRow::Assistant {
                            content: visible, ..
                        }),
                        ProjectionRow::Assistant { content: text, .. },
                    ) if text.starts_with(visible.as_str()) => {
                        *visible = text.clone();
                    }
                    (
                        Some(ProjectionRow::Assistant {
                            content: visible, ..
                        }),
                        ProjectionRow::Assistant { content: text, .. },
                    ) if visible.starts_with(text.as_str()) || visible.ends_with(text) => {}
                    _ => rows.push(streamed),
                }
            }
        }
        if self.chat_state.turn_active()
            && let Some(ProjectionRow::Assistant {
                thought_time_ms, ..
            }) = rows.last_mut()
        {
            *thought_time_ms = self.chat_state.thought_elapsed_ms();
        }
        group_turn_rows(rows)
    }

    fn searchable_rows(rows: &[DisplayRow]) -> Vec<(usize, String)> {
        rows.iter()
            .into_iter()
            .enumerate()
            .flat_map(|(index, row)| match row {
                DisplayRow::User(text) => crate::image_attachment::user_parts(text)
                    .into_iter()
                    .filter_map(|part| match part {
                        crate::image_attachment::UserPart::Text(text) => Some((index, text)),
                        crate::image_attachment::UserPart::Image(_) => None,
                    })
                    .collect(),
                DisplayRow::Turn(parts) => parts
                    .iter()
                    .into_iter()
                    .filter_map(|part| match part {
                        ProjectionRow::Assistant { content, .. } => {
                            let visible = split_thinking(content).0;
                            (!visible.trim().is_empty()).then_some((index, visible))
                        }
                        _ => None,
                    })
                    .collect(),
                DisplayRow::System(_) => Vec::new(),
            })
            .collect()
    }

    fn scroll_to_current_match(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.conversation_search.current_row() {
            self.messages.update(cx, |state, cx| {
                state.scroll_to_item(row, cx);
            });
        }
    }

    fn render_transcript(
        &mut self,
        cx: &mut Context<Self>,
        rows: Vec<DisplayRow>,
    ) -> impl IntoElement {
        let streaming_tail = !self.chat_state.stream_rows().is_empty() && !rows.is_empty();
        if self.messages.read(cx).item_count() != rows.len() {
            self.messages
                .update(cx, |state, cx| state.reset(rows.len(), cx));
        } else if streaming_tail {
            // The streaming tail changes height every frame while the
            // virtual list caches row heights by index. Remeasure it so the
            // transcript never shows stale blank gaps while a turn streams.
            let last = rows.len() - 1;
            self.messages.update(cx, |state, cx| {
                state.remeasure_items(last..last + 1, cx);
            });
        }
        let rendered_rows = rows.clone();
        let expanded_thoughts = self.expanded_thoughts.clone();
        let expanded_tools = self.expanded_tools.clone();
        let current_match_row = self.conversation_search.current_row();
        let mono_font = Theme::global(cx).mono_font_family.clone();
        // Turn timing lives at the end of the last message instead of a
        // fixed row under the transcript.
        let tail_note = self.chat_state.turn_elapsed_ms().map(|elapsed| {
            if self.chat_state.turn_active() {
                format!("Working · {}", format_duration(elapsed))
            } else {
                format!("Turn took {}", format_duration(elapsed))
            }
        });
        let view = cx.entity().downgrade();
        let turn_active = self.chat_state.turn_active();
        MessageScroller::new("conversation", self.messages.clone(), move |index, _, _| {
            let element = match rendered_rows.get(index).cloned() {
                Some(DisplayRow::Turn(parts)) => render_turn(
                    parts,
                    index,
                    expanded_thoughts.clone(),
                    expanded_tools.clone(),
                    mono_font.clone(),
                    view.clone(),
                    turn_active && index + 1 == rendered_rows.len(),
                ),
                Some(DisplayRow::User(text)) => render_user_message(text, index),
                Some(DisplayRow::System(text)) => render_system_message(text, index),
                None => div().child("Message unavailable").into_any_element(),
            };
            let is_last = index + 1 == rendered_rows.len();
            let element = match (&tail_note, is_last) {
                (Some(note), true) => div()
                    .child(element)
                    .child(
                        div()
                            .pt_1()
                            .text_xs()
                            .text_color(rgb(Palette::TEXT_MUTED))
                            .child(note.clone()),
                    )
                    .into_any_element(),
                _ => element,
            };
            if current_match_row == Some(index) {
                div()
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(Palette::APPROVAL))
                    .bg(rgb(Palette::SURFACE_ELEVATED))
                    .child(element)
                    .into_any_element()
            } else {
                element
            }
        })
        // The row wrapper already insets px_3, so keep the viewport flush:
        // a second viewport inset misaligns message text against tool cards.
        // Row gaps come from the row style (the default pb_8 is far too airy).
        .with_content_style(gpui_kit::StyleRefinement::default().px_0().pb_1())
        .with_list_style(gpui_kit::StyleRefinement::default().py_2())
        .with_row_style(gpui_kit::StyleRefinement::default().pb_5())
        .size_full()
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let models = self
            .navigation
            .chat_snapshot
            .as_ref()
            .map(|snapshot| snapshot.models.clone())
            .unwrap_or_default();
        let selected = self.pending_model_selection.clone().or_else(|| {
            self.navigation
                .chat_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.selected_model.clone())
        });
        let selected_label = models
            .iter()
            .find(|model| selected.as_deref() == Some(model.id.as_str()))
            .map(|model| model.label.clone())
            .unwrap_or_else(|| "Choose model".to_owned());
        let view = cx.entity().downgrade();
        Button::new("model-picker")
            .ghost()
            .compact()
            .xsmall()
            .label(selected_label)
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                models.iter().fold(
                    menu.min_w(px(250.)).max_h(px(300.)).scrollable(true),
                    |menu, model| {
                        let model_id = model.id.clone();
                        let view = view.clone();
                        menu.item(
                            PopupMenuItem::new(model.label.clone())
                                .checked(selected.as_deref() == Some(model.id.as_str()))
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        let has_active_session =
                                            this.navigation.chat_snapshot.as_ref().is_some_and(
                                                |snapshot| snapshot.session_id.is_some(),
                                            ) && !this.starting_new_session;
                                        if has_active_session {
                                            this.send_command(
                                                Command::SelectModel(model_id.clone()),
                                                cx,
                                            );
                                        } else {
                                            this.pending_model_selection = Some(model_id.clone());
                                            cx.notify();
                                        }
                                    });
                                }),
                        )
                    },
                )
            })
    }

    fn render_settings_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let narrow = window.bounds().size.width <= px(1000.);
        let state =
            crate::settings::SettingsState::from_snapshot(self.navigation.chat_snapshot.as_ref());
        let models = state.models.clone();
        let selected_model = self
            .pending_model_selection
            .clone()
            .or(state.selected_model.clone());
        let selected_model_label = self
            .pending_model_selection
            .as_deref()
            .and_then(|id| models.iter().find(|model| model.id == id))
            .map(|model| model.label.as_str())
            .or_else(|| state.selected_model_label())
            .map(str::to_owned)
            .unwrap_or_else(|| "No model selected".to_owned());
        let selected_model_id = models
            .iter()
            .find(|model| selected_model.as_deref() == Some(model.id.as_str()))
            .map(|model| model.id.clone());
        let model_picker_view = cx.entity().downgrade();
        let model_picker = Button::new("settings-model-picker")
            .ghost()
            .compact()
            .label(if models.is_empty() {
                "Unavailable"
            } else {
                &selected_model_label
            })
            .dropdown_caret(true)
            .disabled(models.is_empty())
            .accessibility_label("Select model")
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                models.iter().fold(
                    menu.min_w(px(220.)).max_h(px(280.)).scrollable(true),
                    |menu, model| {
                        let model_id = model.id.clone();
                        let view = model_picker_view.clone();
                        menu.item(
                            PopupMenuItem::new(model.label.clone())
                                .checked(selected_model.as_deref() == Some(model.id.as_str()))
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        let has_active_session =
                                            this.navigation.chat_snapshot.as_ref().is_some_and(
                                                |snapshot| snapshot.session_id.is_some(),
                                            ) && !this.starting_new_session;
                                        if has_active_session {
                                            this.send_command(
                                                Command::SelectModel(model_id.clone()),
                                                cx,
                                            );
                                        } else {
                                            this.pending_model_selection = Some(model_id.clone());
                                            cx.notify();
                                        }
                                    });
                                }),
                        )
                    },
                )
            });

        let approval_view = cx.entity().downgrade();
        let current_approval = state.auto_approve;
        let approval_picker = Button::new("settings-approval-picker")
            .ghost()
            .compact()
            .label(state.approval_mode_label())
            .dropdown_caret(true)
            .accessibility_label("Select permission mode")
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                [(false, "Ask first"), (true, "Auto approve")]
                    .into_iter()
                    .fold(menu.min_w(px(180.)), |menu, (auto_approve, label)| {
                        let view = approval_view.clone();
                        menu.item(
                            PopupMenuItem::new(label)
                                .checked(current_approval == auto_approve)
                                .on_click(move |_, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.send_command(
                                            Command::SetAutoApprove(auto_approve),
                                            cx,
                                        );
                                    });
                                }),
                        )
                    })
            });

        let model_support = if let Some(id) = selected_model_id {
            format!("{selected_model_label} · {id}")
        } else if state.has_session {
            "The current session does not report a selectable model.".to_owned()
        } else {
            "Start or open a session to choose a model.".to_owned()
        };
        let approval_support = if state.auto_approve {
            "Tool calls run without a confirmation prompt for this session."
        } else {
            "RustCode asks before running tool calls for this session."
        };

        let rail = div()
            .when(narrow, |this| {
                this.w_full()
                    .pb_4()
                    .border_b_1()
                    .border_color(rgb(Palette::BORDER_SUBTLE))
            })
            .when(!narrow, |this| {
                this.w(px(SETTINGS_RAIL_WIDTH))
                    .flex_shrink_0()
                    .pr_5()
                    .border_r_1()
                    .border_color(rgb(Palette::BORDER_SUBTLE))
            })
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_lg().font_semibold().child("Settings"))
            .child(
                Button::new("settings-section-general")
                    .ghost()
                    .compact()
                    .w_full()
                    .selected(true)
                    .label("General")
                    .icon(IconName::Settings)
                    .accessibility_label("General settings"),
            );

        let model_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
            .py_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_medium().child("Model"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(Palette::TEXT_SECONDARY))
                            .child(model_support),
                    ),
            )
            .child(model_picker);
        let approval_row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
            .py_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().font_medium().child("Permissions"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(Palette::TEXT_SECONDARY))
                            .child(approval_support),
                    ),
            )
            .child(approval_picker);

        let content = div()
            .flex_1()
            .min_w_0()
            .max_w(px(SETTINGS_CONTENT_WIDTH))
            .flex()
            .flex_col()
            .gap_5()
            .child(div().text_2xl().font_semibold().child("General"))
            .child(
                div()
                    .w_full()
                    .px_5()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(Palette::BORDER_SUBTLE))
                    .bg(rgb(Palette::SURFACE_ELEVATED))
                    .child(model_row),
            )
            .child(
                div()
                    .w_full()
                    .px_5()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(Palette::BORDER_SUBTLE))
                    .bg(rgb(Palette::SURFACE_ELEVATED))
                    .child(approval_row),
            );

        div()
            .w_full()
            .h_full()
            .min_h_0()
            .flex()
            .justify_center()
            .px_6()
            .pt(TITLE_BAR_HEIGHT + px(24.))
            .pb_6()
            .child(
                div()
                    .w_full()
                    .max_w(px(SETTINGS_RAIL_WIDTH + SETTINGS_CONTENT_WIDTH + 48.))
                    .h_full()
                    .min_w_0()
                    .flex()
                    .when(narrow, |this| this.flex_col().gap_5())
                    .when(!narrow, |this| this.flex_row().gap_6())
                    .child(rail)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .overflow_scrollbar()
                            .child(content),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let collapsed = self.sidebar_collapsed;
        let project = self
            .navigation
            .chat_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project);
        let project_name = project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose project".to_owned());
        let selected_session = self
            .navigation
            .chat_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.session_id.as_deref());

        let header = div().w_full().flex().flex_col().gap_3().pt(px(30.)).child(
            div()
                .id("new-chat")
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .p_2()
                .rounded_lg()
                .text_sm()
                .cursor_pointer()
                .hover(|this| this.bg(rgb(Palette::SURFACE_HOVER)))
                .child(Icon::new(IconName::Plus).size_4())
                .child("New chat")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_new_chat(cx);
                })),
        );

        let projects = SidebarGroup::new("Projects").child(
            SidebarMenu::new().child(
                SidebarMenuItem::new(project_name)
                    .icon(Icon::new(IconName::FolderOpen))
                    .label_style(gpui_kit::StyleRefinement::default().text_ellipsis())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choose_project_and_start(cx);
                    })),
            ),
        );
        let mut recent_menu = SidebarMenu::new();
        if self.recent_sessions.is_empty() {
            recent_menu =
                recent_menu.child(SidebarMenuItem::new("No recent sessions").disable(true));
        } else {
            for session in &self.recent_sessions {
                let session_id = session.id.clone();
                let title = session.title.clone();
                recent_menu = recent_menu.child(
                    SidebarMenuItem::new(session.title.clone())
                        .icon(Icon::new(IconName::FileText))
                        // The toolkit's label is a flex row whose text child
                        // clips before ellipsis. Give the suffix slot the
                        // remaining width and render a constrained text block.
                        .label_style(gpui_kit::StyleRefinement::default().flex_none().w_0())
                        .suffix(move |_, _| {
                            let tooltip_title = title.clone();
                            div()
                                .id("session-title")
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .tooltip(move |window, cx| {
                                    Tooltip::new(tooltip_title.clone()).build(window, cx)
                                })
                                .child(title.clone())
                        })
                        .active(selected_session == Some(session.id.as_str()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resume_session(session_id.clone(), cx);
                        })),
                );
            }
        }

        let footer = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .pt_2()
            .border_t_1()
            .border_color(rgb(Palette::BORDER_SUBTLE))
            .child(
                Button::new("sidebar-settings")
                    .ghost()
                    .compact()
                    .w_full()
                    .icon(IconName::Settings)
                    .label("Settings")
                    .selected(matches!(
                        self.navigation.destination,
                        AppDestination::Settings(_)
                    ))
                    .accessibility_label("Settings")
                    .tooltip("Settings (⌘,)")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.navigation.open_settings();
                        cx.notify();
                    })),
            );

        Sidebar::new("session-sidebar")
            .w(px(SIDEBAR_WIDTH))
            .bg(rgb(Palette::SIDEBAR))
            .border_color(rgb(Palette::BORDER_SUBTLE))
            .collapsible(SidebarCollapsible::Offcanvas)
            .collapsed(collapsed)
            .header(header)
            .child(projects)
            .child(SidebarGroup::new("Recents").child(recent_menu))
            .footer(footer)
            .into_any_element()
    }

    fn render_question(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let prompt = self.chat_state.pending_question().cloned().or_else(|| {
            self.navigation
                .chat_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_question.clone())
        })?;
        let controller = self.backend.controller().clone();
        let cancel_controller = controller.clone();
        let composer = self.question_answer.clone();
        let view = cx.entity().downgrade();
        let cancel_view = view.clone();
        let options = prompt.options.clone();
        let descriptions = prompt.descriptions.clone();
        let multiple = prompt.multiple;
        Some(
            AlertDialog::new(cx)
                .title(if prompt.header.trim().is_empty() {
                    "The agent has a question".to_owned()
                } else {
                    prompt.header.clone()
                })
                .description(prompt.text)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Answer")
                        .cancel_text("Stop turn")
                        .show_cancel(true),
                )
                .child(Textarea::new(&self.question_answer).h(px(52.)))
                .children(options.into_iter().enumerate().map(|(index, option)| {
                    let answer = option.clone();
                    let controller = controller.clone();
                    let composer = composer.clone();
                    let view = view.clone();
                    let selected = self.selected_question_options.contains(&option);
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            Button::new(format!("question-option-{index}"))
                                .label(option)
                                .selected(selected)
                                .on_click(move |_, window, cx| {
                                    if multiple {
                                        let _ = view.update(cx, |this, cx| {
                                            this.chat_state.begin_user_action();
                                            toggle_option(
                                                &mut this.selected_question_options,
                                                &answer,
                                            );
                                            cx.notify();
                                        });
                                    } else {
                                        let _ = controller
                                            .send(Command::AnswerQuestion(answer.clone()));
                                        let _ = view.update(cx, |this, cx| {
                                            this.chat_state.begin_user_action();
                                            cx.notify();
                                        });
                                        composer.update(cx, |state, cx| {
                                            state.set_value("", window, cx)
                                        });
                                    }
                                }),
                        )
                        .when_some(
                            descriptions
                                .get(index)
                                .filter(|text| !text.trim().is_empty()),
                            |this, description| {
                                this.child(
                                    div()
                                        .pl_2()
                                        .text_xs()
                                        .text_color(rgb(Palette::TEXT_SECONDARY))
                                        .child(description.clone()),
                                )
                            },
                        )
                }))
                .on_ok(move |_, window, app| {
                    let selected = view
                        .upgrade()
                        .map(|view| view.read(app).selected_question_options.clone())
                        .unwrap_or_default();
                    let answer = if multiple && !selected.is_empty() {
                        selected.join(", ")
                    } else {
                        composer.read(app).value().to_string()
                    };
                    if !can_submit(&answer) {
                        return false;
                    }
                    let _ = controller.send(Command::AnswerQuestion(answer));
                    let _ = view.update(app, |this, cx| {
                        this.chat_state.begin_user_action();
                        cx.notify();
                    });
                    composer.update(app, |state, cx| state.set_value("", window, cx));
                    true
                })
                .on_cancel(move |_, _, cx| {
                    let _ = cancel_controller.send(Command::Cancel);
                    let _ = cancel_view.update(cx, |this, cx| {
                        this.chat_state.begin_user_action();
                        cx.notify();
                    });
                    true
                }),
        )
    }

    fn render_approval(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let prompt = self.chat_state.pending_approval().cloned().or_else(|| {
            self.navigation
                .chat_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_approval.clone())
        })?;
        let request_id = prompt.request_id.clone();
        let approve_view = cx.entity().downgrade();
        let deny_view = approve_view.clone();
        let approve_controller = self.backend.controller().clone();
        let deny_controller = approve_controller.clone();
        let approve_id = request_id.clone();
        let deny_id = request_id;
        let in_flight = self.approval_in_flight.is_some();
        Some(
            div()
                .w_full()
                .max_w(px(760.))
                .rounded_lg()
                .border_1()
                .border_color(rgb(Palette::BORDER_SUBTLE))
                .bg(rgb(Palette::SURFACE_ELEVATED))
                .p_4()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().font_semibold().child("Permission required"))
                .child(div().text_sm().child(prompt.action_summary))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(Palette::TEXT_MUTED))
                        .child(prompt.risk_context),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(prompt.description),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("approval-deny")
                                .label(if in_flight { "Denying…" } else { "Deny" })
                                .disabled(in_flight)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        deny_view.update(cx, |this, cx| {
                                            if this.chat_state.pending_approval().is_some_and(
                                                |pending| pending.request_id == deny_id,
                                            ) && this.approval_in_flight.is_none()
                                            {
                                                this.approval_in_flight = Some(deny_id.clone());
                                                this.chat_state.begin_user_action();
                                                this.chat_state.set_approval_denied();
                                                if let Err(error) = deny_controller
                                                    .send(Command::Approval(ApprovalChoice::Deny))
                                                {
                                                    this.approval_in_flight = None;
                                                    this.status = Some(format!(
                                                        "Could not deny approval: {error:?}"
                                                    ));
                                                }
                                                cx.notify();
                                            }
                                        });
                                }),
                        )
                        .child(
                            Button::new("approval-approve")
                                .primary()
                                .label(if in_flight { "Approving…" } else { "Approve" })
                                .disabled(in_flight)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        approve_view.update(cx, |this, cx| {
                                            if this.chat_state.pending_approval().is_some_and(
                                                |pending| pending.request_id == approve_id,
                                            ) && this.approval_in_flight.is_none()
                                            {
                                                this.approval_in_flight = Some(approve_id.clone());
                                                this.chat_state.begin_user_action();
                                                if let Err(error) = approve_controller.send(
                                                    Command::Approval(ApprovalChoice::Approve),
                                                ) {
                                                    this.approval_in_flight = None;
                                                    this.status = Some(format!(
                                                        "Could not approve action: {error:?}"
                                                    ));
                                                }
                                                cx.notify();
                                            }
                                        });
                                }),
                        ),
                ),
        )
    }
}

#[derive(Clone)]
enum DisplayRow {
    User(String),
    Turn(Vec<ProjectionRow>),
    System(String),
}

fn group_turn_rows(rows: Vec<ProjectionRow>) -> Vec<DisplayRow> {
    let mut grouped = Vec::new();
    for row in rows {
        match row {
            ProjectionRow::User(text) => grouped.push(DisplayRow::User(text)),
            ProjectionRow::System(text) => grouped.push(DisplayRow::System(text)),
            part => {
                if let Some(DisplayRow::Turn(parts)) = grouped.last_mut() {
                    parts.push(part);
                } else {
                    grouped.push(DisplayRow::Turn(vec![part]));
                }
            }
        }
    }
    grouped
}

fn format_duration(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{}s", ms / 1_000)
    } else if ms >= 1_000 {
        format!("{:.1}s", ms as f64 / 1_000.)
    } else {
        format!("{ms}ms")
    }
}

fn split_thinking(content: &str) -> (String, Option<(String, bool)>) {
    let mut answer = String::new();
    let mut thoughts = Vec::new();
    let mut remaining = content;
    while let Some(open) = remaining.find("<think>") {
        answer.push_str(&remaining[..open]);
        let after_open = &remaining[open + "<think>".len()..];
        if let Some(close) = after_open.find("</think>") {
            thoughts.push(after_open[..close].trim().to_owned());
            remaining = &after_open[close + "</think>".len()..];
        } else {
            thoughts.push(after_open.trim().to_owned());
            return (answer, Some((thoughts.join("\n\n"), true)));
        }
    }
    answer.push_str(remaining);
    if thoughts.is_empty() {
        (answer, None)
    } else {
        (answer, Some((thoughts.join("\n\n"), false)))
    }
}

fn markdown_style() -> TextViewStyle {
    let mut scrollable_block = gpui_kit::StyleRefinement::default();
    scrollable_block.overflow.x = Some(gpui_kit::Overflow::Scroll);
    TextViewStyle {
        is_dark: true,
        ..TextViewStyle::default()
    }
    .paragraph_gap(gpui_kit::rems(0.75))
    .heading_font_size(|level, _| match level {
        1 => px(21.),
        2 => px(18.),
        _ => px(16.),
    })
    .code_block(scrollable_block.clone())
    .table(scrollable_block)
    .table_head(
        gpui_kit::StyleRefinement::default()
            .bg(rgb(Palette::SURFACE_ELEVATED))
            .text_color(rgb(Palette::TEXT_PRIMARY))
            .font_semibold(),
    )
}

fn tool_summary(tools: &[ProjectionRow]) -> String {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for tool in tools {
        if let ProjectionRow::Tool { name, .. } = tool {
            *counts.entry(name).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(name, count)| {
            let noun = match name {
                "view_file" | "read_file" => "file read",
                "list_directory" => "directory listing",
                "search_files" | "grep_search" => {
                    return format!("{count} search{}", if count == 1 { "" } else { "es" });
                }
                _ => return format!("{count} {}", name.replace('_', " ")),
            };
            format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render_user_message(text: String, index: usize) -> gpui_kit::AnyElement {
    let copy_text = text.clone();
    let parts = crate::image_attachment::user_parts(&text);
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(px(620.))
                .relative()
                .group("user-message")
                .px_4()
                .py_3()
                .rounded_xl()
                .bg(rgb(Palette::SURFACE_COMPOSER))
                .text_size(px(15.))
                .line_height(px(22.))
                .flex()
                .flex_col()
                .gap_2()
                .children(parts.into_iter().enumerate().map(|(part_index, part)| {
                    match part {
                        crate::image_attachment::UserPart::Text(text) => {
                            TextView::markdown(format!("user-{index}-{part_index}"), text)
                                .style(markdown_style())
                                .text_size(px(15.))
                                .line_height(px(22.))
                                .text_color(rgb(Palette::TEXT_PRIMARY))
                                .selectable(true)
                                .into_any_element()
                        }
                        crate::image_attachment::UserPart::Image(path) => {
                            if path.exists() {
                                div()
                                    .size(px(124.))
                                    .rounded_lg()
                                    .overflow_hidden()
                                    .child(
                                        gpui_kit::img(path)
                                            .size_full()
                                            .object_fit(gpui_kit::ObjectFit::Cover),
                                    )
                                    .into_any_element()
                            } else {
                                div()
                                    .text_xs()
                                    .text_color(rgb(Palette::TEXT_MUTED))
                                    .child("Image unavailable")
                                    .into_any_element()
                            }
                        }
                    }
                }))
                .child(
                    div()
                        .h(px(
                            if copy_control_stays_in_hover_region(
                                MESSAGE_COPY_CONTROL_BOTTOM_OFFSET,
                                MESSAGE_COPY_CONTROL_HEIGHT,
                            ) {
                                MESSAGE_COPY_CONTROL_HEIGHT
                            } else {
                                0.
                            },
                        ))
                        .w_full()
                        .flex()
                        .justify_end()
                        .invisible()
                        .group_hover("user-message", |this| this.visible())
                        .child(
                            Button::new(format!("copy-user-message-{index}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .accessibility_label("Copy message")
                                .tooltip("Copy message")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(copy_text.clone().into());
                                }),
                        ),
                ),
        )
        .into_any_element()
}

fn render_system_message(text: String, index: usize) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .text_size(px(14.))
        .text_color(rgb(Palette::TEXT_MUTED))
        .child(
            TextView::markdown(format!("system-{index}"), text)
                .style(markdown_style())
                .text_size(px(13.))
                .text_color(rgb(Palette::TEXT_MUTED)),
        )
        .into_any_element()
}

// Keep commentary on the side of the tools where it was emitted. Each
// segment ends with visible assistant text, so later activity cannot jump
// above an earlier progress update.
fn turn_segments(parts: Vec<ProjectionRow>) -> Vec<Vec<ProjectionRow>> {
    let mut segments = Vec::new();
    let mut pending = Vec::new();
    for part in parts {
        let ends_segment = matches!(&part, ProjectionRow::Assistant { content, .. }
            if !split_thinking(content).0.trim().is_empty());
        pending.push(part);
        if ends_segment {
            segments.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        segments.push(pending);
    }
    segments
}

#[allow(clippy::too_many_arguments)]
fn render_turn(
    parts: Vec<ProjectionRow>,
    index: usize,
    expanded_thoughts: HashSet<(usize, usize)>,
    expanded_tools: HashSet<(usize, usize)>,
    mono_font: gpui_kit::SharedString,
    view: gpui_kit::WeakEntity<AppView>,
    turn_active: bool,
) -> gpui_kit::AnyElement {
    let segments = turn_segments(parts);
    let last_segment = segments.len().saturating_sub(1);
    let mut tool_offset = 0;
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .children(
            segments
                .into_iter()
                .enumerate()
                .map(|(segment_index, parts)| {
                    let offset = tool_offset;
                    tool_offset += parts
                        .iter()
                        .filter(|part| matches!(part, ProjectionRow::Tool { .. }))
                        .count();
                    render_turn_segment(
                        parts,
                        index,
                        segment_index,
                        offset,
                        expanded_thoughts.contains(&(index, segment_index)),
                        expanded_tools.clone(),
                        mono_font.clone(),
                        view.clone(),
                        turn_active && segment_index == last_segment,
                    )
                }),
        )
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_turn_segment(
    parts: Vec<ProjectionRow>,
    index: usize,
    segment_index: usize,
    tool_offset: usize,
    expanded: bool,
    expanded_tools: HashSet<(usize, usize)>,
    mono_font: gpui_kit::SharedString,
    view: gpui_kit::WeakEntity<AppView>,
    turn_active: bool,
) -> gpui_kit::AnyElement {
    let mut answers = Vec::new();
    let mut thoughts = Vec::new();
    let mut tools = Vec::new();
    let mut thought_ms = 0_u64;
    let mut thinking = false;
    for part in parts {
        match part {
            ProjectionRow::Assistant {
                content,
                thought_time_ms,
                ..
            } => {
                let (answer, thought) = split_thinking(&content);
                if !answer.trim().is_empty() {
                    answers.push(answer);
                }
                if let Some((text, ongoing)) = thought {
                    if !text.is_empty() {
                        thoughts.push(text);
                    }
                    thinking = ongoing;
                    thought_ms = thought_ms.saturating_add(thought_time_ms.unwrap_or(0));
                } else {
                    thinking = false;
                }
            }
            tool @ ProjectionRow::Tool { .. } => {
                thinking = false;
                tools.push(tool);
            }
            _ => {}
        }
    }

    thinking &= turn_active
        && !tools.iter().any(|tool| {
            matches!(
                tool,
                ProjectionRow::Tool {
                    status: ToolStatus::Running | ToolStatus::Pending,
                    ..
                }
            )
        });
    let failed_count = tools
        .iter()
        .filter(|tool| {
            matches!(
                tool,
                ProjectionRow::Tool {
                    status: ToolStatus::Failed,
                    ..
                }
            )
        })
        .count();
    let has_activity = !thoughts.is_empty() || !tools.is_empty() || thinking;
    let activity_label = {
        let mut labels = Vec::new();
        if thinking {
            labels.push("Thinking…".to_owned());
        } else if !thoughts.is_empty() {
            labels.push(if thought_ms > 0 {
                format!("Thought for {}", format_duration(thought_ms))
            } else {
                "Thought".to_owned()
            });
        }
        if !tools.is_empty() {
            if tools.iter().any(|tool| {
                matches!(
                    tool,
                    ProjectionRow::Tool {
                        status: ToolStatus::Running | ToolStatus::Pending,
                        ..
                    }
                )
            }) {
                labels.push("Running tools".to_owned());
            }
            labels.push(tool_summary(&tools));
            if failed_count > 0 {
                labels.push(format!("{failed_count} failed"));
            }
        }
        labels.join(" · ")
    };
    let failed = tools.iter().any(|tool| {
        matches!(
            tool,
            ProjectionRow::Tool {
                status: ToolStatus::Failed,
                ..
            }
        )
    });
    let activity = if has_activity {
        let toggle_view = view.clone();
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id(format!("activity-{index}-{segment_index}"))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .text_size(px(13.))
                        .text_color(rgb(if failed { 0xdca5a5 } else { 0x999da5 }))
                        .child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_4(),
                        )
                        .child(
                            div()
                                .with_animation(
                                    format!(
                                        "activity-label-{index}-{segment_index}-{activity_label}"
                                    ),
                                    Animation::new(std::time::Duration::from_millis(140)),
                                    |label, phase| label.opacity(phase),
                                )
                                .child(activity_label),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = toggle_view.update(cx, |this, cx| {
                                let key = (index, segment_index);
                                if !this.expanded_thoughts.insert(key) {
                                    this.expanded_thoughts.remove(&key);
                                }
                                this.messages.update(cx, |state, cx| {
                                    state.remeasure_items(index..index + 1, cx)
                                });
                                cx.notify();
                            });
                        }),
                )
                .when(expanded, |this| {
                    this.child(
                        div()
                            .ml_6()
                            .pl_3()
                            .border_l_1()
                            .border_color(rgb(Palette::BORDER_SUBTLE))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .text_size(px(13.))
                            .text_color(rgb(Palette::TEXT_SECONDARY))
                            .children(thoughts.into_iter().enumerate().map(
                                |(thought_index, thought)| {
                                    TextView::markdown(
                                        format!("thought-{index}-{segment_index}-{thought_index}"),
                                        thought,
                                    )
                                    .style(markdown_style())
                                    .text_size(px(13.))
                                    .line_height(px(19.))
                                    .font_weight(gpui_kit::FontWeight::NORMAL)
                                    .text_color(rgb(Palette::TEXT_SECONDARY))
                                    .into_any_element()
                                },
                            ))
                            .children(tools.into_iter().enumerate().map(|(tool_index, tool)| {
                                render_tool_detail(
                                    tool,
                                    index,
                                    tool_offset + tool_index,
                                    expanded_tools.contains(&(index, tool_offset + tool_index)),
                                    mono_font.clone(),
                                    view.clone(),
                                )
                            })),
                    )
                }),
        )
    } else {
        None
    };

    div()
        .w_full()
        .max_w(px(740.))
        .flex()
        .flex_col()
        .gap_3()
        .text_color(rgb(Palette::TEXT_PRIMARY))
        .when(segment_index == 0, |this| {
            this.child(
                div()
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(rgb(Palette::TEXT_MUTED))
                    .child("RustCode"),
            )
        })
        .when_some(activity, |this, activity| this.child(activity))
        .children(
            answers
                .into_iter()
                .enumerate()
                .map(|(answer_index, answer)| {
                    let copy_text = answer.clone();
                    div()
                        .w_full()
                        .min_w_0()
                        .relative()
                        .group("assistant-message")
                        .text_size(px(15.))
                        .line_height(px(23.))
                        .child(
                            TextView::markdown(
                                format!("assistant-{index}-{segment_index}-{answer_index}"),
                                answer,
                            )
                            .style(markdown_style())
                            .text_size(px(15.))
                            .line_height(px(23.))
                            .font_weight(gpui_kit::FontWeight::NORMAL)
                            .text_color(rgb(Palette::TEXT_PRIMARY)),
                        )
                        .child(
                            div()
                                .h(px(
                                    if copy_control_stays_in_hover_region(
                                        MESSAGE_COPY_CONTROL_BOTTOM_OFFSET,
                                        MESSAGE_COPY_CONTROL_HEIGHT,
                                    ) {
                                        MESSAGE_COPY_CONTROL_HEIGHT
                                    } else {
                                        0.
                                    },
                                ))
                                .w_full()
                                .flex()
                                .justify_end()
                                .invisible()
                                .group_hover("assistant-message", |this| this.visible())
                                .child(
                                    Button::new(format!(
                                        "copy-assistant-{index}-{segment_index}-{answer_index}"
                                    ))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .accessibility_label("Copy reply")
                                    .tooltip("Copy reply")
                                    .on_click(
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(copy_text.clone().into());
                                        },
                                    ),
                                ),
                        )
                }),
        )
        .into_any_element()
}

fn render_tool_detail(
    tool: ProjectionRow,
    turn_index: usize,
    tool_index: usize,
    expanded: bool,
    mono_font: gpui_kit::SharedString,
    view: gpui_kit::WeakEntity<AppView>,
) -> gpui_kit::AnyElement {
    let ProjectionRow::Tool {
        name,
        detail,
        content,
        status,
        elapsed_ms,
    } = tool
    else {
        unreachable!();
    };
    let presentation = tool_row_presentation(status, &content);
    let (icon, color) = match status {
        ToolStatus::Running => (IconName::LoaderCircle, 0xc9a76b),
        ToolStatus::Pending => (IconName::Pause, 0xc9a76b),
        ToolStatus::Completed => (IconName::CircleCheck, 0x91b89b),
        ToolStatus::Failed => (IconName::CircleX, 0xd88d8d),
    };
    let status_icon = Icon::new(icon).size_4().text_color(rgb(color));
    let status_icon = if status == ToolStatus::Running {
        status_icon
            .with_animation(
                format!("tool-running-{turn_index}-{tool_index}"),
                Animation::new(std::time::Duration::from_millis(900))
                    .repeat()
                    .with_max_fps(30.),
                |icon, phase| icon.rotate(gpui_kit::radians(phase * std::f32::consts::TAU)),
            )
            .into_any_element()
    } else {
        status_icon.into_any_element()
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(format!("tool-{turn_index}-{tool_index}"))
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size_4(),
                )
                .child(status_icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(match detail {
                            Some(detail) => format!("{} · {detail}", name.replace('_', " ")),
                            None => name.replace('_', " "),
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(color))
                        .with_animation(
                            format!(
                                "tool-status-{turn_index}-{tool_index}-{}",
                                presentation.status_label
                            ),
                            Animation::new(std::time::Duration::from_millis(140)),
                            |label, phase| label.opacity(phase),
                        )
                        .child(presentation.status_label),
                )
                .when_some(elapsed_ms, |this, ms| {
                    this.child(div().text_xs().child(format_duration(ms)))
                })
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        let key = (turn_index, tool_index);
                        if !this.expanded_tools.insert(key) {
                            this.expanded_tools.remove(&key);
                        }
                        this.messages.update(cx, |state, cx| {
                            state.remeasure_items(turn_index..turn_index + 1, cx)
                        });
                        cx.notify();
                    });
                }),
        )
        .when(expanded && presentation.output_expandable, |this| {
            this.child(
                div()
                    .ml_6()
                    .max_h(px(180.))
                    .overflow_scrollbar()
                    .min_h(px(0.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(Palette::BORDER_SUBTLE))
                    .bg(rgb(Palette::SURFACE_ELEVATED))
                    .px_3()
                    .py_2()
                    .child(
                        TextView::markdown(
                            format!("tool-output-{turn_index}-{tool_index}"),
                            content,
                        )
                        .text_size(px(12.))
                        .text_color(rgb(Palette::TEXT_MUTED))
                        .font_family(mono_font)
                        .selectable(true),
                    ),
            )
        })
        .into_any_element()
}

fn should_show_start_screen(snapshot: Option<&ControllerSnapshot>) -> bool {
    !snapshot.is_some_and(|snapshot| snapshot.session_id.is_some())
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dialogs = Root::render_dialog_layer(window, cx);
        if self.reset_search_input_on_render {
            self.search_input
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.reset_search_input_on_render = false;
        }
        if self.focus_search_on_render {
            let focus_handle = self.search_input.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus_handle, cx));
            self.focus_search_on_render = false;
        }
        if self.focus_composer_on_render {
            let focus_handle = self.composer.read(cx).focus_handle(cx);
            window.on_next_frame(move |window, cx| window.focus(&focus_handle, cx));
            self.focus_composer_on_render = false;
        }
        if self.clear_composer_on_render {
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.clear_composer_on_render = false;
        }
        let workspace = self
            .navigation
            .chat_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project)
            .display()
            .to_string();
        let project_label = self
            .navigation
            .chat_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose project".to_owned());
        let status = self.status.clone();
        let composer_action = self.chat_state.composer_action();
        let auto_approve = self
            .navigation
            .chat_snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.auto_approve);
        let composer_enabled = self.chat_state.composer_enabled();
        let pending_question = self.chat_state.pending_question().is_some()
            || self
                .navigation
                .chat_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pending_question.is_some());
        let send_enabled = composer_enabled
            && self.pending_prompt.is_none()
            && !self.starting_new_session
            && (can_submit(&self.composer.read(cx).value())
                || (!pending_question && !self.pending_images.is_empty()));
        let slash_suggestions = crate::slash::suggestions(&self.composer.read(cx).value());
        self.slash_selection = self
            .slash_selection
            .min(slash_suggestions.len().saturating_sub(1));
        let has_session = !should_show_start_screen(self.navigation.chat_snapshot.as_ref());
        let conversation_title = self.navigation.chat_snapshot.as_ref().and_then(|snapshot| {
            let id = snapshot.session_id.as_ref()?;
            self.recent_sessions
                .iter()
                .find(|session| &session.id == id)
                .map(|session| session.title.clone())
                .or_else(|| {
                    snapshot
                        .transcript
                        .iter()
                        .find(|item| item.role == "user")
                        .map(|item| item.content.lines().next().unwrap_or("").to_owned())
                })
        });
        let sidebar = self.render_sidebar(cx);

        let welcome = div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_3xl()
                    .font_medium()
                    .child("What would you like to build?"),
            )
            .child(
                div()
                    .text_base()
                    .text_color(rgb(Palette::TEXT_MUTED))
                    .child("Choose a project, then ask RustCode to get started."),
            );

        let center = if has_session {
            let rows = self.display_rows();
            self.conversation_search
                .update_rows(&Self::searchable_rows(&rows));
            let search_bar = self
                .chat_search_open
                .then(|| self.render_chat_search(cx).into_any_element());
            let transcript = self.render_transcript(cx, rows);
            div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .when_some(search_bar, |this, bar| this.child(bar))
                .child(transcript)
                .into_any_element()
        } else {
            welcome.into_any_element()
        };
        let context_row = div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .text_xs()
            .text_color(rgb(Palette::TEXT_SECONDARY))
            .child(
                Button::new("composer-project")
                    .ghost()
                    .compact()
                    .xsmall()
                    .icon(IconName::FolderOpen)
                    .label(project_label)
                    .tooltip(workspace)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.choose_project_and_start(cx);
                    })),
            )
            .when_some(self.git_branch.clone(), |this, branch| {
                this.child(Icon::new(IconName::Network).size_4())
                    .child(branch)
            });
        let paste_view = cx.entity().downgrade();
        let composer = div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .bg(rgb(Palette::SURFACE_COMPOSER))
            .border_1()
            .border_color(rgb(Palette::BORDER_SUBTLE))
            .rounded(px(23.))
            .when(!self.pending_images.is_empty(), |this| {
                this.child(
                    div().flex().flex_wrap().gap_2().children(
                        self.pending_images
                            .iter()
                            .enumerate()
                            .map(|(index, image)| {
                                let view = cx.entity().downgrade();
                                div()
                                    .relative()
                                    .size(px(68.))
                                    .rounded_lg()
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(rgb(Palette::BORDER_STRONG))
                                    .child(
                                        gpui_kit::img(image.path.clone())
                                            .size_full()
                                            .object_fit(gpui_kit::ObjectFit::Cover),
                                    )
                                    .child(
                                        Button::new(format!("remove-image-{index}"))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Close)
                                            .accessibility_label("Remove image")
                                            .absolute()
                                            .top_0()
                                            .right_0()
                                            .tooltip("Remove image")
                                            .on_click(move |_, _, cx| {
                                                let _ = view.update(cx, |this, cx| {
                                                    this.pending_images.remove(index);
                                                    cx.notify();
                                                });
                                            }),
                                    )
                            }),
                    ),
                )
            })
            .child(
                div()
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let draft = this.composer.read(cx).value().to_string();
                        let composer_focused = this
                            .composer
                            .read(cx)
                            .presentation()
                            .focus_handle()
                            .is_focused(window);
                        match slash_menu_key_decision(
                            &draft,
                            this.slash_selection,
                            this.slash_picker_dismissed,
                            &event.keystroke.key,
                            composer_focused,
                        ) {
                            crate::slash::SlashInteraction::Move(index) => {
                                this.slash_selection = index;
                                cx.stop_propagation();
                                cx.notify();
                            }
                            crate::slash::SlashInteraction::Dismiss => {
                                this.slash_picker_dismissed = true;
                                cx.stop_propagation();
                                cx.notify();
                            }
                            crate::slash::SlashInteraction::Complete { .. }
                            | crate::slash::SlashInteraction::Ignore => {}
                        }
                    }))
                    .on_action(cx.listener(|this, action: &Enter, window, cx| {
                        if !action.shift && !action.secondary {
                            if this.submit_composer(window, cx) {
                                cx.stop_propagation();
                            }
                        }
                    }))
                    .child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .bordered(false)
                            .disabled(!composer_enabled)
                            .when(
                                crate::slash::is_recognized_command(
                                    &self.composer.read(cx).value(),
                                ),
                                |this| {
                                    this.text_color(rgb(Palette::COMMAND_ACCENT))
                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                },
                            )
                            .on_paste(move |item, _, cx| {
                                if pending_question
                                    && crate::image_attachment::contains_images(item)
                                {
                                    let _ = paste_view.update(cx, |this, cx| {
                                        this.status = Some(
                                            "Answer the question before attaching an image"
                                                .to_owned(),
                                        );
                                        cx.notify();
                                    });
                                    return true;
                                }
                                match crate::image_attachment::paste_images(item) {
                                    Ok(None) => false,
                                    Ok(Some(images)) => {
                                        let _ = paste_view.update(cx, |this, cx| {
                                            this.pending_images.extend(images);
                                            this.status = None;
                                            cx.notify();
                                        });
                                        true
                                    }
                                    Err(error) => {
                                        let _ = paste_view.update(cx, |this, cx| {
                                            this.status = Some(error);
                                            cx.notify();
                                        });
                                        true
                                    }
                                }
                            }),
                    ),
            )
            .when_some(status.clone(), |this, message| {
                this.child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(rgb(Palette::DESTRUCTIVE))
                        .child(message),
                )
            })
            .when_some(self.chat_state.error().map(str::to_owned), |this, error| {
                this.child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(rgb(Palette::DESTRUCTIVE))
                        .child(error),
                )
            })
            .when_some(self.chat_state.queued_message_label(), |this, label| {
                this.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(Palette::TEXT_SECONDARY))
                        .child(label),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div().flex_1().flex().items_center().child(
                            div()
                                .id("auto-approve")
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_1()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_xs()
                                .hover(|this| this.bg(rgb(Palette::SURFACE_HOVER)))
                                .child(
                                    Icon::new(if auto_approve {
                                        IconName::CircleCheck
                                    } else {
                                        IconName::CircleX
                                    })
                                    .size_4(),
                                )
                                .child(if auto_approve {
                                    "Auto approve"
                                } else {
                                    "Ask first"
                                })
                                .tooltip(move |window, cx| {
                                    Tooltip::new(if auto_approve {
                                    "Tool actions are approved automatically. Click to ask first."
                                } else {
                                    "Tool actions ask for approval. Click to auto approve."
                                }).build(window, cx)
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let enabled = this
                                        .navigation
                                        .chat_snapshot
                                        .as_ref()
                                        .is_none_or(|snapshot| snapshot.auto_approve);
                                    this.send_command(Command::SetAutoApprove(!enabled), cx);
                                })),
                        ),
                    )
                    .child(self.render_model_picker(cx))
                    .child(
                        Button::new("composer-action")
                            .primary()
                            .rounded(px(999.))
                            .size(px(32.))
                            .when(composer_action == ComposerAction::Stop, |this| {
                                this.child(
                                    div()
                                        .size(px(10.))
                                        .rounded(px(2.))
                                        .bg(Theme::global(cx).button_primary_foreground),
                                )
                            })
                            .when(composer_action == ComposerAction::Send, |this| {
                                this.icon(IconName::ArrowUp)
                            })
                            .accessibility_label(match composer_action {
                                ComposerAction::Stop => "Stop turn",
                                ComposerAction::Send if pending_question => "Answer",
                                ComposerAction::Send => "Send message",
                            })
                            .tooltip(match composer_action {
                                ComposerAction::Stop => "Stop turn",
                                ComposerAction::Send if pending_question => "Answer",
                                ComposerAction::Send => "Send message",
                            })
                            .disabled(composer_action == ComposerAction::Send && !send_enabled)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if composer_action == ComposerAction::Stop {
                                    this.send_command(Command::Cancel, cx);
                                } else {
                                    this.submit_composer(window, cx);
                                }
                            })),
                    ),
            );

        let chat_main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .px(px(MAIN_PANE_INSET))
            .pt(TITLE_BAR_HEIGHT)
            .pb_3()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(center),
            )
            .when_some(self.render_question(cx), |this, question| {
                this.child(question)
            })
            .when_some(self.render_approval(cx), |this, approval| {
                this.child(approval)
            })
            .when_some(self.chat_state.approval_status(), |this, message| {
                this.child(div().w_full().max_w(px(760.)).text_sm().child(message))
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .child(context_row)
                    .child(composer)
                    .when(
                        !slash_suggestions.is_empty() && !self.slash_picker_dismissed,
                        |this| {
                            this.child(
                                div()
                                    .w(px(460.))
                                    .max_w(px(460.))
                                    .absolute()
                                    .left_0()
                                    .bottom_full()
                                    .mb_2()
                                    .p_2()
                                    .rounded(px(23.))
                                    .border_1()
                                    .border_color(rgb(Palette::BORDER_STRONG))
                                    .bg(rgb(Palette::SURFACE_ELEVATED))
                                    .max_h(px(300.))
                                    .children(slash_suggestions.into_iter().enumerate().map(
                                        |(index, suggestion)| {
                                            let completion =
                                                crate::slash::complete(suggestion.name);
                                            let view = cx.entity().downgrade();
                                            let hover_view = view.clone();
                                            div()
                                                .id(format!("slash-{}", suggestion.name))
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap_3()
                                                .px_2()
                                                .py_1()
                                                .rounded_lg()
                                                .when(index == self.slash_selection, |this| {
                                                    this.bg(rgb(Palette::SURFACE_SELECTED))
                                                })
                                                .cursor_pointer()
                                                .hover(|this| this.bg(rgb(Palette::SURFACE_HOVER)))
                                                .on_hover(move |hovered, _, cx| {
                                                    if !*hovered {
                                                        return;
                                                    }
                                                    let _ = hover_view.update(cx, |this, cx| {
                                                        this.slash_selection = index;
                                                        cx.notify();
                                                    });
                                                })
                                                .child(
                                                    div()
                                                        .min_w(px(112.))
                                                        .font_medium()
                                                        .child(suggestion.name),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(Palette::TEXT_SECONDARY))
                                                        .child(suggestion.description),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        let cursor_offset =
                                                            completion.chars().count();
                                                        this.apply_slash_completion(
                                                            completion.clone(),
                                                            cursor_offset,
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                })
                                        },
                                    )),
                            )
                        },
                    ),
            );
        let main = match self.navigation.destination {
            AppDestination::Chat => chat_main.into_any_element(),
            AppDestination::Settings(SettingsSection::General) => {
                self.render_settings_page(window, cx)
            }
        };

        let toggle_icon = IconName::PanelLeft;
        let title_bar = TitleBar::new()
            .bg(gpui_kit::rgba(0x00000000))
            .border_color(rgb(Palette::BORDER_SUBTLE))
            .child(
                div()
                    .h_full()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(TITLE_BAR_CHILD_GAP))
                    .child(
                        Button::new("sidebar-toggle")
                            .ghost()
                            .with_size(px(SIDEBAR_TOGGLE_SIZE))
                            .icon(toggle_icon)
                            .text_color(rgb(Palette::TEXT_MUTED))
                            .tooltip("Toggle sidebar")
                            .accessibility_label("Toggle sidebar")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
                    )
                    .when_some(conversation_title.filter(|_| has_session), |this, title| {
                        this.child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .ml(if self.sidebar_collapsed {
                                    px(0.)
                                } else {
                                    px(SIDEBAR_TITLE_MARGIN)
                                })
                                .text_sm()
                                .font_medium()
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .size_4()
                                        .text_color(rgb(Palette::TEXT_SECONDARY)),
                                )
                                .child(div().flex_1().min_w_0().text_ellipsis().child(title)),
                        )
                    }),
            );

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(|_: &MinimizeWindow, window, _| window.minimize_window())
            .flex()
            .bg(rgb(Palette::APP_BACKGROUND))
            .text_color(rgb(Palette::TEXT_PRIMARY))
            .child(div().size_full().flex().child(sidebar).child(main))
            .child(div().absolute().top_0().left_0().right_0().child(title_bar))
            .children(dialogs)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustcode::controller::{
        ControllerError, ControllerSnapshot, SessionChoice, TranscriptItem,
    };

    use super::{
        AppDestination, AppNavigation, ChatViewState, ControllerUpdate, DisplayRow, ProjectionRow,
        SettingsSection, ToolStatus, group_turn_rows, should_show_start_screen,
        slash_menu_key_decision, turn_segments,
    };

    #[test]
    fn copy_control_region_remains_inside_the_message_hover_group() {
        assert!(super::copy_control_stays_in_hover_region(
            super::MESSAGE_COPY_CONTROL_BOTTOM_OFFSET,
            super::MESSAGE_COPY_CONTROL_HEIGHT,
        ));
        assert!(!super::copy_control_stays_in_hover_region(-27., 0.));
    }

    #[test]
    fn replacing_pending_approval_releases_only_the_old_in_flight_request() {
        assert_eq!(
            super::approval_in_flight_after_snapshot(Some("request-a".into()), Some("request-a")),
            Some("request-a".into()),
            "the same request remains disabled while its decision resolves"
        );
        assert_eq!(
            super::approval_in_flight_after_snapshot(Some("request-a".into()), Some("request-b")),
            None,
            "a replacement request must not inherit the old in-flight state"
        );
        assert_eq!(
            super::approval_in_flight_after_snapshot(Some("request-a".into()), None),
            None,
            "a resolved approval clears the in-flight state"
        );
    }

    #[test]
    fn composer_routes_gpui_arrow_names_to_slash_navigation() {
        assert_eq!(
            slash_menu_key_decision("/", 0, false, "ArrowDown", true),
            crate::slash::SlashInteraction::Move(1)
        );
        assert_eq!(
            slash_menu_key_decision("ordinary text", 0, false, "ArrowDown", true),
            crate::slash::SlashInteraction::Ignore
        );
        assert_eq!(
            slash_menu_key_decision("/", 0, false, "ArrowDown", false),
            crate::slash::SlashInteraction::Ignore
        );
    }

    #[test]
    fn settings_destination_returns_to_chat_without_replacing_the_active_session() {
        let mut navigation = AppNavigation::new(Some(ControllerSnapshot {
            generation: 7,
            workspace: Some(PathBuf::from("/workspace")),
            session_id: Some("session-42".to_owned()),
            sessions: Vec::new(),
            models: Vec::new(),
            selected_model: None,
            transcript: vec![TranscriptItem {
                role: "user".to_owned(),
                content: "keep this conversation".to_owned(),
                tool_name: None,
                tool_detail: None,
                tool_success: None,
                tool_pending: false,
                response_time_ms: None,
                thought_time_ms: None,
            }],
            live_response: String::new(),
            queued_count: 0,
            turn_active: false,
            auto_approve: false,
            pending_question: None,
            pending_approval: None,
        }));

        navigation.open_settings();
        assert_eq!(
            navigation.destination,
            AppDestination::Settings(SettingsSection::General)
        );

        navigation.open_chat();
        assert_eq!(navigation.destination, AppDestination::Chat);
        let snapshot = navigation.chat_snapshot.as_ref().expect("active session");
        assert_eq!(snapshot.session_id.as_deref(), Some("session-42"));
        assert_eq!(snapshot.transcript[0].content, "keep this conversation");
    }

    #[test]
    fn adjacent_assistant_activity_stays_in_one_turn() {
        let rows = group_turn_rows(vec![
            ProjectionRow::User("question".into()),
            ProjectionRow::Assistant {
                content: "<think>inspect</think>".into(),
                response_time_ms: None,
                thought_time_ms: Some(100),
            },
            ProjectionRow::Tool {
                name: "view_file".into(),
                detail: None,
                content: "file output".into(),
                status: ToolStatus::Completed,
                elapsed_ms: Some(10),
            },
            ProjectionRow::Assistant {
                content: "Answer".into(),
                response_time_ms: Some(200),
                thought_time_ms: None,
            },
            ProjectionRow::User("follow up".into()),
        ]);
        assert_eq!(rows.len(), 3);
        assert!(matches!(&rows[0], DisplayRow::User(text) if text == "question"));
        assert!(matches!(&rows[1], DisplayRow::Turn(parts) if parts.len() == 3));
        assert!(matches!(&rows[2], DisplayRow::User(text) if text == "follow up"));
    }

    #[test]
    fn commentary_stays_before_the_tools_it_introduces() {
        let before = ProjectionRow::Assistant {
            content: "I will inspect the file.".into(),
            response_time_ms: None,
            thought_time_ms: None,
        };
        let tool = ProjectionRow::Tool {
            name: "view_file".into(),
            detail: None,
            content: "file".into(),
            status: ToolStatus::Completed,
            elapsed_ms: None,
        };
        let after = ProjectionRow::Assistant {
            content: "Here is the result.".into(),
            response_time_ms: None,
            thought_time_ms: None,
        };
        assert_eq!(
            turn_segments(vec![before.clone(), tool.clone(), after.clone()]),
            vec![vec![before], vec![tool, after]]
        );
    }

    #[test]
    fn list_sessions_snapshot_with_launch_workspace_keeps_start_screen_visible() {
        let initial = ControllerSnapshot {
            generation: 0,
            workspace: Some(PathBuf::from("/launch")),
            session_id: None,
            sessions: Vec::new(),
            models: Vec::new(),
            selected_model: None,
            transcript: Vec::new(),
            live_response: String::new(),
            queued_count: 0,
            turn_active: false,
            auto_approve: true,
            pending_question: None,
            pending_approval: None,
        };
        let listed = ControllerSnapshot {
            sessions: vec![SessionChoice {
                id: "saved-session".to_owned(),
                title: "Saved session".to_owned(),
                when: "today".to_owned(),
                message_count: 2,
            }],
            ..initial.clone()
        };
        let active = ControllerSnapshot {
            session_id: Some("active-session".to_owned()),
            ..listed.clone()
        };

        assert!(should_show_start_screen(Some(&initial)));
        assert!(should_show_start_screen(Some(&listed)));
        assert!(!should_show_start_screen(Some(&active)));
    }

    #[test]
    fn invalid_workspace_error_is_available_to_the_start_screen() {
        let mut chat_state = ChatViewState::default();
        chat_state.apply_update(ControllerUpdate::Error(ControllerError::InvalidWorkspace(
            "workspace must be an existing directory".into(),
        )));

        assert_eq!(
            chat_state.error(),
            Some("Controller error: InvalidWorkspace(\"workspace must be an existing directory\")")
        );
    }
}
