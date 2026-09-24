use std::path::PathBuf;

use gpui_kit::{
    Context, PathPromptOptions, Render, SharedString, Window,
    component::{
        Disableable, Selectable, StyledExt,
        bubble::Bubble,
        button::{Button, ButtonVariants},
        dialog::{AlertDialog, DialogButtonProps},
        input::{Textarea, TextareaState},
        message::{Message, MessageAlignment, MessageContent, MessageHeader},
        message_scroller::{MessageScroller, MessageScrollerState},
        text::TextView,
    },
    div,
    prelude::*,
    px,
};
use rustcode::controller::{
    ApprovalChoice, Command, ControllerEvent, ControllerSnapshot, ControllerUpdate,
};

use crate::{
    backend::{NativeBackend, project_selection_command, resume_session_command},
    projection::{
        ChatViewState, ProjectionRow, can_submit, project_rows, stop_available, toggle_option,
    },
};

pub struct AppView {
    backend: NativeBackend,
    launch_dir: PathBuf,
    composer: gpui_kit::Entity<TextareaState>,
    question_answer: gpui_kit::Entity<TextareaState>,
    messages: gpui_kit::Entity<MessageScrollerState>,
    snapshot: Option<ControllerSnapshot>,
    chat_state: ChatViewState,
    selected_question_options: Vec<String>,
    status: Option<String>,
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
        let composer = cx.new(|cx| TextareaState::new(window, cx));
        let question_answer = cx.new(|cx| TextareaState::new(window, cx));
        let messages = cx.new(|cx| MessageScrollerState::new(0, cx));
        Self {
            backend,
            launch_dir,
            composer,
            question_answer,
            messages,
            snapshot: None,
            chat_state: ChatViewState::default(),
            selected_question_options: Vec::new(),
            status,
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

        self.chat_state.apply_update(event.update.clone());
        match event.update {
            ControllerUpdate::Snapshot(snapshot) => {
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
            }
            ControllerUpdate::Turn(_) => {}
            ControllerUpdate::Error(_) => {
                self.status = None;
            }
        }
        cx.notify();
    }

    pub fn controller_stopped(&mut self, cx: &mut Context<Self>) {
        self.status = Some("Controller update stream closed".to_owned());
        cx.notify();
    }

    fn start_workspace(&mut self, workspace: PathBuf, cx: &mut Context<Self>) {
        if let Some(command) = project_selection_command(Some(workspace)) {
            self.send_command(command, cx);
        }
    }

    fn resume_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let command = resume_session_command(session_id, self.launch_dir.clone());
        self.send_command(command, cx);
    }

    fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).value().to_string();
        if !can_submit(&text) {
            return;
        }
        let command = if let Some(question) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pending_question.as_ref())
        {
            crate::projection::answer_for_question(question, None, &text)
                .map(Command::AnswerQuestion)
        } else {
            Some(Command::Submit(text))
        };
        if let Some(command) = command {
            self.send_command(command, cx);
            self.composer
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
    }

    fn send_command(&mut self, command: Command, cx: &mut Context<Self>) {
        self.chat_state.begin_user_action();
        if let Err(error) = self.backend.controller().send(command) {
            let message = format!("Controller error: {error:?}");
            self.chat_state.set_error(message);
            self.status = None;
            cx.notify();
        }
    }

    fn render_transcript(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = self
            .snapshot
            .as_ref()
            .map(|snapshot| project_rows(&snapshot.transcript, &snapshot.live_response))
            .unwrap_or_default();
        if self.chat_state.turn_active() {
            for streamed in self.chat_state.stream_rows() {
                match (rows.last_mut(), streamed) {
                    (Some(ProjectionRow::Assistant(visible)), ProjectionRow::Assistant(text))
                        if text.starts_with(visible.as_str()) =>
                    {
                        *visible = text.clone();
                    }
                    (Some(ProjectionRow::Assistant(visible)), ProjectionRow::Assistant(text))
                        if visible.starts_with(text.as_str()) || visible.ends_with(text) => {}
                    _ => rows.push(streamed.clone()),
                }
            }
        }
        if self.messages.read(cx).item_count() != rows.len() {
            self.messages
                .update(cx, |state, cx| state.reset(rows.len(), cx));
        }
        let rendered_rows = rows.clone();
        MessageScroller::new("conversation", self.messages.clone(), move |index, _, _| {
            render_message(
                rendered_rows
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| ProjectionRow::System("Message unavailable".to_owned())),
            )
        })
        .with_content_style(gpui_kit::StyleRefinement::default().gap_4().p_4())
        .size_full()
    }

    fn render_model_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let models = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.models.clone())
            .unwrap_or_default();
        let selected = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.selected_model.clone());
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(models.into_iter().map(|model| {
                let model_id = model.id.clone();
                Button::new(format!("model-{}", model.id))
                    .label(model.label)
                    .selected(selected.as_deref() == Some(model_id.as_str()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send_command(Command::SelectModel(model_id.clone()), cx);
                    }))
            }))
    }

    fn render_question(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let prompt = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pending_question.clone())?;
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
        let prompt = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pending_approval.clone())?;
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

fn render_message(row: ProjectionRow) -> impl IntoElement {
    match row {
        ProjectionRow::User(text) => Message::new()
            .alignment(MessageAlignment::End)
            .header(MessageHeader::new().child("You"))
            .content(MessageContent::new().bubble(Bubble::new().child(text))),
        ProjectionRow::Assistant(text) => Message::new()
            .alignment(MessageAlignment::Start)
            .header(MessageHeader::new().child("RustCode"))
            .content(MessageContent::new().child(TextView::markdown("assistant-message", text))),
        ProjectionRow::Tool { name, content } => Message::new()
            .alignment(MessageAlignment::Start)
            .header(MessageHeader::new().child(format!("Tool · {name}")))
            .content(MessageContent::new().bubble(Bubble::new().child(content))),
        ProjectionRow::System(text) => Message::new()
            .alignment(MessageAlignment::Start)
            .header(MessageHeader::new().child("System"))
            .content(MessageContent::new().child(TextView::markdown("system-message", text))),
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let launch_dir = self.launch_dir.display().to_string();
        let workspace = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .map(|path| path.display().to_string());
        let status = self.status.clone().unwrap_or_default();
        let turn_active = self.chat_state.turn_active();
        let composer_enabled = self.chat_state.composer_enabled();
        let send_enabled = composer_enabled && can_submit(&self.composer.read(cx).value());

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(div().text_2xl().font_semibold().child("RustCode"))
            .child(
                div().text_sm().child(
                    workspace
                        .clone()
                        .unwrap_or_else(|| "No project selected".to_owned()),
                ),
            )
            .child(self.render_model_buttons(cx));

        let shell = if workspace.is_none() {
            let sessions = self
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.sessions.clone())
                .unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().child(format!("Launch directory: {launch_dir}")))
                .child(
                    Button::new("start-launch-directory")
                        .primary()
                        .label("Begin in launch directory")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_workspace(this.launch_dir.clone(), cx);
                        })),
                )
                .child(div().child("Or choose a project directory:"))
                .child(
                    Button::new("choose-project")
                        .label("Choose project folder")
                        .on_click(cx.listener(|_, _, _, cx| {
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
                                        if let Some(command) = paths
                                            .into_iter()
                                            .next()
                                            .and_then(|path| project_selection_command(Some(path)))
                                        {
                                            this.send_command(command, cx);
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
                        })),
                )
                .child(div().font_semibold().child("Recent sessions"))
                .child(if sessions.is_empty() {
                    div().text_sm().child("No saved sessions yet.")
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(sessions.into_iter().map(|session| {
                            let session_id = session.id.clone();
                            let label = format!(
                                "{} · {} · {} messages",
                                session.title, session.when, session.message_count
                            );
                            Button::new(format!("resume-session-{}", session.id))
                                .label(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.resume_session(session_id.clone(), cx);
                                }))
                        }))
                })
        } else {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap_3()
                .child(self.render_transcript(cx))
                .when_some(self.render_question(cx), |this, question| {
                    this.child(question)
                })
                .when_some(self.render_approval(cx), |this, approval| {
                    this.child(approval)
                })
                .child(
                    div()
                        .flex()
                        .items_end()
                        .gap_2()
                        .child(
                            Textarea::new(&self.composer)
                                .h(px(96.))
                                .disabled(!composer_enabled)
                                .flex_1(),
                        )
                        .when(stop_available(turn_active), |this| {
                            this.child(Button::new("stop-turn").label("Stop").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.send_command(Command::Cancel, cx);
                                }),
                            ))
                        })
                        .child(
                            Button::new("send-message")
                                .primary()
                                .disabled(!send_enabled)
                                .label(
                                    if self
                                        .snapshot
                                        .as_ref()
                                        .is_some_and(|snapshot| snapshot.pending_question.is_some())
                                    {
                                        "Answer"
                                    } else {
                                        "Send"
                                    },
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_composer(window, cx);
                                })),
                        ),
                )
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .child(header)
            .child(shell)
            .child(div().text_sm().child(SharedString::from(status)))
            .when_some(self.chat_state.error().map(str::to_owned), |this, error| {
                this.child(div().text_sm().child(error))
            })
            .when_some(self.chat_state.approval_status(), |this, status| {
                this.child(div().text_sm().child(status))
            })
    }
}

#[cfg(test)]
mod tests {
    use rustcode::controller::ControllerError;

    use super::{ChatViewState, ControllerUpdate};

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
