use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use gpui_kit::{
    Anchor, Context, PathPromptOptions, Render, Window, actions,
    component::{
        Disableable, Icon, IconName, Selectable, StyledExt, TitleBar,
        bubble::Bubble,
        button::{Button, ButtonVariants},
        dialog::{AlertDialog, DialogButtonProps},
        input::{Enter, Textarea, TextareaState},
        menu::{DropdownMenu, PopupMenuItem},
        message::{Message, MessageAlignment, MessageContent, MessageHeader},
        message_scroller::{MessageScroller, MessageScrollerState},
        sidebar::{
            Sidebar, SidebarCollapsible, SidebarGroup, SidebarMenu, SidebarMenuItem,
            SidebarToggleButton,
        },
        text::TextView,
    },
    div,
    prelude::*,
    px, rgb,
};
use rustcode::controller::{
    ApprovalChoice, Command, ControllerEvent, ControllerSnapshot, ControllerUpdate, SessionChoice,
};

actions!(rustcode_app, [ToggleSidebar]);

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

use crate::{
    backend::{
        NativeBackend, project_selection_command, resolve_resume_workspace, resume_session_command,
    },
    projection::{
        ChatViewState, ProjectionRow, ToolStatus, can_submit, project_rows, stop_available,
        toggle_option,
    },
};

pub struct AppView {
    backend: NativeBackend,
    launch_dir: PathBuf,
    selected_project: PathBuf,
    composer: gpui_kit::Entity<TextareaState>,
    question_answer: gpui_kit::Entity<TextareaState>,
    messages: gpui_kit::Entity<MessageScrollerState>,
    snapshot: Option<ControllerSnapshot>,
    recent_sessions: Vec<SessionChoice>,
    chat_state: ChatViewState,
    selected_question_options: Vec<String>,
    status: Option<String>,
    pending_prompt: Option<String>,
    pending_model_selection: Option<String>,
    starting_new_session: bool,
    clear_composer_on_render: bool,
    sidebar_collapsed: bool,
    git_branch: Option<String>,
    expanded_thoughts: HashSet<usize>,
    turn_timer_epoch: u64,
}

impl AppView {
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
                .auto_grow(2, 6)
                .submit_on_enter(true)
        });
        let question_answer = cx.new(|cx| TextareaState::new(window, cx));
        let messages = cx.new(|cx| MessageScrollerState::new(0, cx));
        Self {
            backend,
            git_branch: current_branch(&launch_dir),
            expanded_thoughts: HashSet::new(),
            turn_timer_epoch: 0,
            selected_project: launch_dir.clone(),
            launch_dir,
            composer,
            question_answer,
            messages,
            snapshot: None,
            recent_sessions: Vec::new(),
            chat_state: ChatViewState::default(),
            selected_question_options: Vec::new(),
            status,
            pending_prompt: None,
            pending_model_selection: None,
            starting_new_session: false,
            clear_composer_on_render: false,
            sidebar_collapsed: false,
        }
    }

    pub fn take_updates(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<ControllerEvent> {
        self.backend.take_updates()
    }

    pub fn apply_event(&mut self, event: ControllerEvent, cx: &mut Context<Self>) {
        if self
            .snapshot
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

        self.chat_state.apply_update(event.update.clone());
        match event.update {
            ControllerUpdate::Snapshot(snapshot) => {
                if !snapshot.sessions.is_empty() {
                    self.recent_sessions = snapshot.sessions.clone();
                }
                let prior_session_id = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.session_id.clone());
                let prior_generation = self.snapshot.as_ref().map(|snapshot| snapshot.generation);
                let session_id = snapshot.session_id.clone();
                let started_session = session_id.is_some()
                    && (session_id != prior_session_id
                        || prior_generation != Some(snapshot.generation));
                let previous_question = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.pending_question.as_ref())
                    .map(|question| question.text.as_str());
                let next_question = snapshot
                    .pending_question
                    .as_ref()
                    .map(|question| question.text.as_str());
                if previous_question != next_question {
                    self.selected_question_options.clear();
                }
                self.snapshot = Some(snapshot);
                self.status = None;
                if started_session {
                    self.expanded_thoughts.clear();
                    self.chat_state.clear_turn_elapsed();
                    self.starting_new_session = false;
                }
                if let Some(workspace) = self
                    .snapshot
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

    fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_prompt.is_some()
            || self.starting_new_session
            || !self.chat_state.composer_enabled()
        {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        if !can_submit(&text) {
            return;
        }
        let command = if let Some(question) = self.chat_state.pending_question().or_else(|| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_question.as_ref())
        }) {
            crate::projection::answer_for_question(question, None, &text)
                .map(Command::AnswerQuestion)
        } else {
            if self
                .snapshot
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
        }
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

    fn render_transcript(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = self
            .snapshot
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
            && thought_time_ms.is_none()
        {
            *thought_time_ms = self.chat_state.thought_elapsed_ms();
        }
        let rows = group_tool_rows(rows);
        if self.messages.read(cx).item_count() != rows.len() {
            self.messages
                .update(cx, |state, cx| state.reset(rows.len(), cx));
        }
        let rendered_rows = rows.clone();
        let expanded_thoughts = self.expanded_thoughts.clone();
        let view = cx.entity().downgrade();
        MessageScroller::new("conversation", self.messages.clone(), move |index, _, _| {
            match rendered_rows.get(index).cloned() {
                Some(DisplayRow::Message(row)) => {
                    render_message(row, index, expanded_thoughts.contains(&index), view.clone())
                }
                Some(DisplayRow::ToolGroup(tools)) => render_tool_group(tools),
                None => div().child("Message unavailable").into_any_element(),
            }
        })
        .with_content_style(gpui_kit::StyleRefinement::default().gap_3().px_4().pb_4())
        .size_full()
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let models = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.models.clone())
            .unwrap_or_default();
        let selected = self.pending_model_selection.clone().or_else(|| {
            self.snapshot
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
                                            this.snapshot.as_ref().is_some_and(|snapshot| {
                                                snapshot.session_id.is_some()
                                            }) && !this.starting_new_session;
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let collapsed = self.sidebar_collapsed;
        let project = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project);
        let project_name = project
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose project".to_owned());
        let selected_session = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.session_id.as_deref());

        let header = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .pt(px(34.))
            .child(div().font_semibold().child("RustCode"))
            .child(
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
                    .hover(|this| this.bg(rgb(0x34363a)))
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
                recent_menu = recent_menu.child(
                    SidebarMenuItem::new(session.title.clone())
                        .icon(Icon::new(IconName::FileText))
                        .label_style(gpui_kit::StyleRefinement::default().text_ellipsis())
                        .active(selected_session == Some(session.id.as_str()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resume_session(session_id.clone(), cx);
                        })),
                );
            }
        }

        Sidebar::new("session-sidebar")
            .w(px(270.))
            .bg(rgb(0x222426))
            .border_color(rgb(0x34363a))
            .collapsible(SidebarCollapsible::Offcanvas)
            .collapsed(collapsed)
            .header(header)
            .child(projects)
            .child(SidebarGroup::new("Recents").child(recent_menu))
            .into_any_element()
    }

    fn render_question(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let prompt = self.chat_state.pending_question().cloned().or_else(|| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_question.clone())
        })?;
        let controller = self.backend.controller().clone();
        let composer = self.question_answer.clone();
        let view = cx.entity().downgrade();
        let options = prompt.options.clone();
        let multiple = prompt.multiple;
        Some(
            AlertDialog::new(cx)
                .title("The agent has a question")
                .description(prompt.text)
                .button_props(DialogButtonProps::default().ok_text("Answer"))
                .child(Textarea::new(&self.question_answer).h(px(100.)))
                .children(options.into_iter().enumerate().map(|(index, option)| {
                    let answer = option.clone();
                    let controller = controller.clone();
                    let composer = composer.clone();
                    let view = view.clone();
                    let selected = self.selected_question_options.contains(&option);
                    Button::new(format!("question-option-{index}"))
                        .label(option)
                        .selected(selected)
                        .on_click(move |_, window, cx| {
                            if multiple {
                                let _ = view.update(cx, |this, cx| {
                                    this.chat_state.begin_user_action();
                                    toggle_option(&mut this.selected_question_options, &answer);
                                    cx.notify();
                                });
                            } else {
                                let _ = controller.send(Command::AnswerQuestion(answer.clone()));
                                let _ = view.update(cx, |this, cx| {
                                    this.chat_state.begin_user_action();
                                    cx.notify();
                                });
                                composer.update(cx, |state, cx| state.set_value("", window, cx));
                            }
                        })
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
                }),
        )
    }

    fn render_approval(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let prompt = self.chat_state.pending_approval().cloned().or_else(|| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pending_approval.clone())
        })?;
        let controller = self.backend.controller().clone();
        let approve_controller = controller.clone();
        let view = cx.entity().downgrade();
        let approve_view = view.clone();
        Some(
            AlertDialog::new(cx)
                .title(format!("Approve {}?", prompt.tool_name))
                .description(prompt.description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Approve")
                        .cancel_text("Deny")
                        .show_cancel(true),
                )
                .on_ok(move |_, _, cx| {
                    let _ = approve_controller.send(Command::Approval(ApprovalChoice::Approve));
                    let _ = approve_view.update(cx, |this, cx| {
                        this.chat_state.begin_user_action();
                        cx.notify();
                    });
                    true
                })
                .on_cancel(move |_, _, cx| {
                    let _ = controller.send(Command::Approval(ApprovalChoice::Deny));
                    let _ = view.update(cx, |this, cx| {
                        this.chat_state.begin_user_action();
                        this.chat_state.set_approval_denied();
                        cx.notify();
                    });
                    true
                }),
        )
    }
}

#[derive(Clone)]
enum DisplayRow {
    Message(ProjectionRow),
    ToolGroup(Vec<ProjectionRow>),
}

fn group_tool_rows(rows: Vec<ProjectionRow>) -> Vec<DisplayRow> {
    let mut grouped = Vec::new();
    for row in rows {
        if matches!(row, ProjectionRow::Tool { .. }) {
            if let Some(DisplayRow::ToolGroup(tools)) = grouped.last_mut() {
                tools.push(row);
            } else {
                grouped.push(DisplayRow::ToolGroup(vec![row]));
            }
        } else {
            grouped.push(DisplayRow::Message(row));
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

fn render_tool_group(tools: Vec<ProjectionRow>) -> gpui_kit::AnyElement {
    let count = tools.len();
    div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(rgb(0x25272a))
        .when(count > 1, |this| {
            this.child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(rgb(0x9da0a8))
                    .child(format!("{count} tool calls")),
            )
        })
        .children(tools.into_iter().filter_map(|tool| {
            let ProjectionRow::Tool {
                name,
                status,
                elapsed_ms,
                ..
            } = tool
            else {
                return None;
            };
            let (label, icon, color) = match status {
                ToolStatus::Running => ("Running", IconName::LoaderCircle, 0xc9a76b),
                ToolStatus::Pending => ("Pending", IconName::Pause, 0xc9a76b),
                ToolStatus::Completed => ("Done", IconName::CircleCheck, 0x91b89b),
                ToolStatus::Failed => ("Failed", IconName::CircleX, 0xd88d8d),
            };
            Some(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(Icon::new(icon).size_4().text_color(rgb(color)))
                    .child(div().flex_1().min_w_0().text_ellipsis().child(name))
                    .child(div().text_xs().text_color(rgb(color)).child(label))
                    .when_some(elapsed_ms, |this, elapsed| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x8c8f98))
                                .child(format_duration(elapsed)),
                        )
                    }),
            )
        }))
        .into_any_element()
}

fn render_message(
    row: ProjectionRow,
    index: usize,
    thought_expanded: bool,
    view: gpui_kit::WeakEntity<AppView>,
) -> gpui_kit::AnyElement {
    match row {
        ProjectionRow::User(text) => Message::new()
            .alignment(MessageAlignment::End)
            .header(MessageHeader::new().child("You"))
            .content(MessageContent::new().bubble(Bubble::new().child(text)))
            .into_any_element(),
        ProjectionRow::Assistant {
            content,
            response_time_ms,
            thought_time_ms,
        } => {
            let (answer, thought) = split_thinking(&content);
            let header = response_time_ms
                .map(|ms| format!("RustCode · {} response", format_duration(ms)))
                .unwrap_or_else(|| "RustCode".to_owned());
            let body = div()
                .flex()
                .flex_col()
                .gap_2()
                .when_some(thought, |this, (thought, ongoing)| {
                    let label = if ongoing {
                        thought_time_ms
                            .map(|ms| format!("Thinking · {}", format_duration(ms)))
                            .unwrap_or_else(|| "Thinking…".to_owned())
                    } else if let Some(ms) = thought_time_ms {
                        format!("Thought for {}", format_duration(ms))
                    } else {
                        "Thought".to_owned()
                    };
                    let view = view.clone();
                    this.child(
                        Button::new(format!("thought-{index}"))
                            .ghost()
                            .compact()
                            .justify_start()
                            .icon(if thought_expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .label(label)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    if !this.expanded_thoughts.insert(index) {
                                        this.expanded_thoughts.remove(&index);
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .when(thought_expanded && !thought.is_empty(), |this| {
                        this.child(
                            div()
                                .max_w(px(760.))
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .bg(rgb(0x25272a))
                                .text_color(rgb(0xb7bac2))
                                .child(TextView::markdown(
                                    format!("thought-content-{index}"),
                                    thought,
                                )),
                        )
                    })
                })
                .when(!answer.trim().is_empty(), |this| {
                    this.child(TextView::markdown(format!("assistant-{index}"), answer))
                });
            Message::new()
                .alignment(MessageAlignment::Start)
                .header(MessageHeader::new().child(header))
                .content(MessageContent::new().child(body))
                .into_any_element()
        }
        ProjectionRow::Tool { .. } => render_tool_group(vec![row]),
        ProjectionRow::System(text) => Message::new()
            .alignment(MessageAlignment::Start)
            .header(MessageHeader::new().child("System"))
            .content(
                MessageContent::new().child(TextView::markdown(format!("system-{index}"), text)),
            )
            .into_any_element(),
    }
}

fn should_show_start_screen(snapshot: Option<&ControllerSnapshot>) -> bool {
    !snapshot.is_some_and(|snapshot| snapshot.session_id.is_some())
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_composer_on_render {
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
            self.clear_composer_on_render = false;
        }
        let workspace = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project)
            .display()
            .to_string();
        let project_label = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .unwrap_or(&self.selected_project)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Choose project".to_owned());
        let status = self.status.clone();
        let turn_active = self.chat_state.turn_active();
        let auto_approve = self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.auto_approve);
        let composer_enabled = self.chat_state.composer_enabled();
        let send_enabled = composer_enabled
            && self.pending_prompt.is_none()
            && !self.starting_new_session
            && can_submit(&self.composer.read(cx).value());
        let has_session = !should_show_start_screen(self.snapshot.as_ref());
        let pending_question = self.chat_state.pending_question().is_some()
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.pending_question.is_some());

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
                    .text_color(rgb(0x92949e))
                    .child("Choose a project, then ask RustCode to get started."),
            );

        let center = if has_session {
            self.render_transcript(cx).into_any_element()
        } else {
            welcome.into_any_element()
        };
        let context_row = div()
            .w_full()
            .max_w(px(860.))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_1()
            .text_xs()
            .text_color(rgb(0xb5b7bd))
            .child(
                Button::new("composer-project")
                    .ghost()
                    .compact()
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
        let composer = div()
            .w_full()
            .max_w(px(860.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(0x2b2c30))
            .border_1()
            .border_color(rgb(0x383a40))
            .rounded_xl()
            .child(
                div()
                    .on_action(cx.listener(|this, action: &Enter, window, cx| {
                        if !action.shift && !action.secondary {
                            this.submit_composer(window, cx);
                        }
                    }))
                    .child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .bordered(false)
                            .disabled(!composer_enabled),
                    ),
            )
            .when_some(status.clone(), |this, message| {
                this.child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(rgb(0xf0a0a0))
                        .child(message),
                )
            })
            .when_some(self.chat_state.error().map(str::to_owned), |this, error| {
                this.child(
                    div()
                        .px_1()
                        .text_sm()
                        .text_color(rgb(0xf0a0a0))
                        .child(error),
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
                            Button::new("auto-approve")
                                .ghost()
                                .compact()
                                .w(px(155.))
                                .justify_start()
                                .icon(if auto_approve {
                                    IconName::CircleCheck
                                } else {
                                    IconName::CircleX
                                })
                                .label(if auto_approve {
                                    "Auto approve"
                                } else {
                                    "Ask first"
                                })
                                .tooltip(if auto_approve {
                                    "Tool actions are approved automatically. Click to ask first."
                                } else {
                                    "Tool actions ask for approval. Click to auto approve."
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let enabled = this
                                        .snapshot
                                        .as_ref()
                                        .is_none_or(|snapshot| snapshot.auto_approve);
                                    this.send_command(Command::SetAutoApprove(!enabled), cx);
                                })),
                        ),
                    )
                    .child(self.render_model_picker(cx))
                    .when(stop_available(turn_active), |this| {
                        this.child(
                            Button::new("stop-turn")
                                .ghost()
                                .icon(IconName::CircleX)
                                .accessibility_label("Stop turn")
                                .tooltip("Stop turn")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send_command(Command::Cancel, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("send-message")
                            .primary()
                            .icon(IconName::ArrowUp)
                            .rounded(px(999.))
                            .accessibility_label(if pending_question {
                                "Answer"
                            } else {
                                "Send message"
                            })
                            .disabled(!send_enabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_composer(window, cx)),
                            ),
                    ),
            );

        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .px_6()
            .pt(px(34.))
            .pb_5()
            .child(
                div()
                    .w_full()
                    .max_w(px(1040.))
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
                this.child(div().w_full().max_w(px(860.)).text_sm().child(message))
            })
            .when_some(self.chat_state.turn_elapsed_ms(), |this, elapsed| {
                this.child(
                    div()
                        .w_full()
                        .max_w(px(860.))
                        .text_xs()
                        .text_color(rgb(0x8c8f98))
                        .child(if turn_active {
                            format!("Working · {}", format_duration(elapsed))
                        } else {
                            format!("Turn took {}", format_duration(elapsed))
                        }),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(860.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(context_row)
                    .child(composer),
            );

        let title_bar = TitleBar::new()
            .bg(gpui_kit::rgba(0x00000000))
            .border_color(gpui_kit::rgba(0x00000000))
            .child(
                SidebarToggleButton::new()
                    .collapsed(self.sidebar_collapsed)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            );

        div()
            .size_full()
            .relative()
            .flex()
            .bg(rgb(0x1b1d1f))
            .text_color(rgb(0xe8e9ed))
            .child(div().size_full().flex().child(sidebar).child(main))
            .child(div().absolute().top_0().left_0().right_0().child(title_bar))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustcode::controller::{ControllerError, ControllerSnapshot, SessionChoice};

    use super::{ChatViewState, ControllerUpdate, should_show_start_screen};

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
