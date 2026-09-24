use std::path::PathBuf;

use gpui_kit::{
    Context, Render, SharedString, Window,
    component::{
        button::{Button, ButtonVariants},
        input::{Input, InputState},
    },
    div,
    prelude::*,
    px,
};
use rustcode::controller::{Command, ControllerEvent, ControllerSnapshot, ControllerUpdate};

use crate::backend::NativeBackend;

pub struct AppView {
    backend: NativeBackend,
    launch_dir: PathBuf,
    project_path: gpui_kit::Entity<InputState>,
    snapshot: Option<ControllerSnapshot>,
    status: Option<String>,
}

impl AppView {
    pub fn new(
        backend: NativeBackend,
        launch_dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_path = cx.new(|cx| InputState::new(window, cx));
        Self {
            backend,
            launch_dir,
            project_path,
            snapshot: None,
            status: None,
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

        match event.update {
            ControllerUpdate::Snapshot(snapshot) => {
                self.snapshot = Some(snapshot);
                self.status = None;
            }
            ControllerUpdate::Turn(update) => {
                self.status = Some(format!("{update:?}"));
            }
            ControllerUpdate::Error(error) => {
                self.status = Some(format!("Controller error: {error:?}"));
            }
        }
        cx.notify();
    }

    pub fn controller_stopped(&mut self, cx: &mut Context<Self>) {
        self.status = Some("Controller update stream closed".to_owned());
        cx.notify();
    }

    fn start_workspace(&mut self, workspace: PathBuf, cx: &mut Context<Self>) {
        if let Err(error) = self.backend.controller().send(Command::StartNew(workspace)) {
            self.status = Some(format!("Controller error: {error:?}"));
            cx.notify();
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let launch_dir = self.launch_dir.display().to_string();
        let workspace = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.workspace.as_ref())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "No project selected".to_owned());
        let status = self.status.clone().unwrap_or_default();

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .child(div().text_2xl().child("RustCode"))
            .child(div().child(format!("Launch directory: {launch_dir}")))
            .child(div().child(workspace))
            .child(
                Button::new("start-launch-directory")
                    .primary()
                    .label("Begin in launch directory")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_workspace(this.launch_dir.clone(), cx);
                    })),
            )
            .child(div().child("Or enter a project directory:"))
            .child(Input::new(&self.project_path).w(px(480.)))
            .child(
                Button::new("choose-project")
                    .label("Choose project")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let project = this.project_path.read(cx).value().to_string();
                        if project.trim().is_empty() {
                            this.status = Some("Enter a project directory first".to_owned());
                            cx.notify();
                        } else {
                            this.start_workspace(PathBuf::from(project), cx);
                        }
                    })),
            )
            .child(div().child(SharedString::from(status)))
    }
}
