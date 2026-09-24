use std::path::PathBuf;

use rustcode::controller::{ControllerEvent, ControllerHandle, InteractiveController};
use tokio::{runtime::Runtime, sync::mpsc};

/// Owns the async runtime and the UI-neutral session controller.
pub struct NativeBackend {
    _runtime: Runtime,
    controller: ControllerHandle,
    updates: Option<mpsc::UnboundedReceiver<ControllerEvent>>,
}

impl NativeBackend {
    pub fn new(launch_dir: PathBuf) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed to start Tokio runtime: {error}"))?;
        let (controller, updates) = InteractiveController::spawn(runtime.handle(), launch_dir);
        Ok(Self {
            _runtime: runtime,
            controller,
            updates: Some(updates),
        })
    }

    pub fn controller(&self) -> &ControllerHandle {
        &self.controller
    }

    pub fn take_updates(&mut self) -> mpsc::UnboundedReceiver<ControllerEvent> {
        self.updates
            .take()
            .expect("controller updates are taken only once")
    }

    #[cfg(test)]
    fn runtime(&self) -> &Runtime {
        &self._runtime
    }
}

impl Drop for NativeBackend {
    fn drop(&mut self) {
        let _ = self
            .controller
            .send(rustcode::controller::Command::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustcode::controller::{Command, ControllerUpdate};

    use super::NativeBackend;

    #[test]
    fn starts_empty_and_accepts_start_new_without_a_terminal() {
        let launch_dir = std::env::current_dir().expect("current directory");
        let mut backend =
            NativeBackend::new(launch_dir.clone()).expect("controller backend starts");
        let mut updates = backend.take_updates();
        let initial = backend
            .runtime()
            .block_on(updates.recv())
            .expect("initial snapshot");

        assert!(matches!(
            initial.update,
            ControllerUpdate::Snapshot(snapshot)
                if snapshot.session_id.is_none()
                    && snapshot.workspace.is_none()
                    && snapshot.transcript.is_empty()
        ));

        backend
            .controller()
            .send(Command::StartNew(PathBuf::from(&launch_dir)))
            .expect("start new session command");
        let started = backend
            .runtime()
            .block_on(updates.recv())
            .expect("started snapshot");

        assert!(matches!(
            started.update,
            ControllerUpdate::Snapshot(snapshot)
                if snapshot.session_id.is_some()
                    && snapshot.workspace.is_some()
        ));
    }
}
