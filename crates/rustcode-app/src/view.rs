use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use gpui_kit::{
    Anchor, Context, PathPromptOptions, Render, Window, actions,
    component::{
        Disableable, Icon, IconName, Selectable, Sizable, StyledExt, Theme, TitleBar,
        button::{Button, ButtonVariants},
        dialog::{AlertDialog, DialogButtonProps},
        input::{Enter, Textarea, TextareaState},
        menu::{DropdownMenu, PopupMenuItem},
        message_scroller::{MessageScroller, MessageScrollerState},
        scroll::ScrollableElement,
        sidebar::{Sidebar, SidebarCollapsible, SidebarGroup, SidebarMenu, SidebarMenuItem},
        text::{TextView, TextViewStyle},
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
    expanded_thoughts: HashSet<(usize, usize)>,
    expanded_tools: HashSet<(usize, usize)>,
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
            expanded_tools: HashSet::new(),
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
                    self.expanded_tools.clear();
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
        {
            *thought_time_ms = self.chat_state.thought_elapsed_ms();
        }
        let rows = group_turn_rows(rows);
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
                Some(DisplayRow::User(text)) => render_user_message(text),
                Some(DisplayRow::System(text)) => render_system_message(text, index),
                None => div().child("Message unavailable").into_any_element(),
            };
            let is_last = index + 1 == rendered_rows.len();
            match (&tail_note, is_last) {
                (Some(note), true) => div()
                    .child(element)
                    .child(
                        div()
                            .pt_1()
                            .text_xs()
                            .text_color(rgb(0x8c8f98))
                            .child(note.clone()),
                    )
                    .into_any_element(),
                _ => element,
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
    TextViewStyle::default()
        .paragraph_gap(gpui_kit::rems(0.75))
        .heading_font_size(|level, _| match level {
            1 => px(21.),
            2 => px(18.),
            _ => px(16.),
        })
        .code_block(scrollable_block.clone())
        .table(scrollable_block)
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

fn render_user_message(text: String) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(
            div()
                .max_w(px(620.))
                .px_4()
                .py_3()
                .rounded_xl()
                .bg(rgb(0x303236))
                .text_size(px(15.))
                .line_height(px(22.))
                .child(text),
        )
        .into_any_element()
}

fn render_system_message(text: String, index: usize) -> gpui_kit::AnyElement {
    div()
        .w_full()
        .text_size(px(14.))
        .text_color(rgb(0x9a9da5))
        .child(TextView::markdown(format!("system-{index}"), text).style(markdown_style()))
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
                        .child(activity_label)
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
                            .border_color(rgb(0x383b40))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .text_size(px(13.))
                            .text_color(rgb(0xa4a7ae))
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
                                    .text_color(rgb(0xa4a7ae))
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
        .text_color(rgb(0xdfe1e5))
        .when(segment_index == 0, |this| {
            this.child(
                div()
                    .text_size(px(12.))
                    .font_medium()
                    .text_color(rgb(0x92969e))
                    .child("RustCode"),
            )
        })
        .when_some(activity, |this, activity| this.child(activity))
        .children(
            answers
                .into_iter()
                .enumerate()
                .map(|(answer_index, answer)| {
                    div()
                        .w_full()
                        .min_w_0()
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
                            .text_color(rgb(0xdfe1e5)),
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
        content,
        status,
        elapsed_ms,
    } = tool
    else {
        unreachable!();
    };
    let (icon, status_label, color) = match status {
        ToolStatus::Running => (IconName::LoaderCircle, "Running", 0xc9a76b),
        ToolStatus::Pending => (IconName::Pause, "Pending", 0xc9a76b),
        ToolStatus::Completed => (IconName::CircleCheck, "Done", 0x91b89b),
        ToolStatus::Failed => (IconName::CircleX, "Failed", 0xd88d8d),
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
                .child(Icon::new(icon).size_4().text_color(rgb(color)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_ellipsis()
                        .child(name.replace('_', " ")),
                )
                .child(div().text_xs().text_color(rgb(color)).child(status_label))
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
        .when(expanded && !content.trim().is_empty(), |this| {
            this.child(
                div()
                    .ml_6()
                    .max_h(px(180.))
                    .overflow_scrollbar()
                    .font_family(mono_font)
                    .text_size(px(12.))
                    .text_color(rgb(0x92969e))
                    .child(content),
            )
        })
        .into_any_element()
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
            .max_w(px(760.))
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
        let composer = div()
            .w_full()
            .max_w(px(760.))
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
                                .xsmall()
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
                                .primary()
                                .rounded(px(999.))
                                .size(px(32.))
                                .child(
                                    div()
                                        .size(px(10.))
                                        .rounded(px(2.))
                                        .bg(Theme::global(cx).button_primary_foreground),
                                )
                                .accessibility_label("Stop turn")
                                .tooltip("Stop turn")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.send_command(Command::Cancel, cx);
                                })),
                        )
                    })
                    .when(!turn_active || send_enabled, |this| {
                        this.child(
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
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_composer(window, cx)
                                })),
                        )
                    }),
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
            .pt(px(16.))
            .pb_5()
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
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(context_row)
                    .child(composer),
            );

        // SidebarToggleButton hardcodes a small button and a 16px icon with
        // no size override, so use a ghost button directly. Large (24px
        // icon) reads chunky next to the traffic lights; a 28px box lands
        // the icon at ~21px, between small and large.
        let toggle_icon = if self.sidebar_collapsed {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        let title_bar = TitleBar::new()
            .bg(gpui_kit::rgba(0x00000000))
            .border_color(gpui_kit::rgba(0x00000000))
            .child(
                Button::new("sidebar-toggle")
                    .ghost()
                    .with_size(px(28.))
                    .icon(toggle_icon)
                    .tooltip("Toggle sidebar")
                    .accessibility_label("Toggle sidebar")
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

    use super::{
        ChatViewState, ControllerUpdate, DisplayRow, ProjectionRow, ToolStatus, group_turn_rows,
        should_show_start_screen, turn_segments,
    };

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
