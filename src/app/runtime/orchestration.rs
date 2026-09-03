use super::*;
use rustcode_tasks::TaskEvent;
use std::sync::mpsc::TryRecvError;

const IDLE_SUMMARY_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn record_active_background_task(
    state: &mut crate::app::AppState,
    task_id: &str,
    output: crate::tools::ToolExecutionOutput,
) -> bool {
    if state.background_wakeup_ids.contains(task_id) {
        return false;
    }
    state
        .history
        .push(crate::background_task_history_message(task_id, output));
    crate::queue_background_wakeup(state, task_id);
    true
}

async fn apply_background_task_event(
    app_state: &std::sync::Arc<tokio::sync::Mutex<crate::app::AppState>>,
    event: TaskEvent,
) {
    let Some((task_id, session_id, output)) = crate::tools::task_event_to_tool_output(event) else {
        return;
    };
    let mut state = app_state.lock().await;
    if state.active_session_id == session_id {
        if record_active_background_task(&mut state, &task_id, output) {
            crate::config::save_session_history(&session_id, &state.history);
        }
    } else {
        let mut history = crate::config::load_session_history_direct(&session_id);
        history.push(crate::background_task_history_message(&task_id, output));
        crate::config::save_session_history(&session_id, &history);
    }
}

impl AppRuntime {
    pub(crate) async fn run(self) -> Result<crate::ExitSummary, Box<dyn Error>> {
        let AppRuntime {
            terminal_runtime,
            app_state,
            client,
            current_cancel_token,
            needs_redraw,
            was_responding,
            terminal_focused,
            transcript_cursor,
            transcript_state,
            stream_commits,
            replaying_transcript,
            terminal_size,
            tui_events,
            frame_requester,
            frame_stream,
            app_event_sender,
            app_event_receiver,
            agent_ui_event_sender,
            agent_ui_event_receiver,
            task_subscriptions,
        } = self;
        let mut terminal_runtime = terminal_runtime
            .ok_or_else(|| Box::<dyn Error>::from("interactive terminal is unavailable"))?;
        let mut current_cancel_token = current_cancel_token;
        let mut needs_redraw = needs_redraw;
        let mut was_responding = was_responding;
        let mut terminal_focused = terminal_focused;
        let mut transcript_cursor = transcript_cursor;
        let mut transcript_state = transcript_state;
        let mut stream_commits = stream_commits;
        let mut replaying_transcript = replaying_transcript;
        let mut terminal_size = terminal_size;
        let mut tui_events = tui_events;
        let mut frame_stream = frame_stream;
        let mut app_event_receiver = app_event_receiver;
        let mut agent_ui_event_receiver = agent_ui_event_receiver;
        let mut task_subscriptions = task_subscriptions;
        let update_exit;
        let mut last_progress_sent = std::time::Instant::now();
        let composer = ui::Composer::new();
        loop {
            let active_session_id = app_state.lock().await.active_session_id.clone();
            task_subscriptions
                .entry(active_session_id.clone())
                .or_insert_with(|| {
                    crate::tools::background_task_manager()
                        .subscribe_session(active_session_id.clone())
                });
            let mut task_events = Vec::new();
            let mut disconnected_sessions = Vec::new();
            for (session_id, subscription) in &mut task_subscriptions {
                loop {
                    match subscription.try_recv() {
                        Ok(event) => task_events.push(event),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            disconnected_sessions.push(session_id.clone());
                            break;
                        }
                    }
                }
            }
            for event in task_events {
                apply_background_task_event(&app_state, event).await;
                needs_redraw = true;
            }
            let manager = crate::tools::background_task_manager();
            // A task removes its record immediately before publishing the
            // terminal event. `has_running` is synchronized with that
            // publication, but drain once more before pruning an inactive
            // subscription so the event that made the session quiescent is
            // consumed instead of being dropped with its receiver.
            let mut late_task_events = Vec::new();
            let mut late_disconnected_sessions = Vec::new();
            for (session_id, subscription) in &mut task_subscriptions {
                if session_id == &active_session_id || manager.has_running(session_id) {
                    continue;
                }
                loop {
                    match subscription.try_recv() {
                        Ok(event) => late_task_events.push(event),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            late_disconnected_sessions.push(session_id.clone());
                            break;
                        }
                    }
                }
            }
            for event in late_task_events {
                apply_background_task_event(&app_state, event).await;
                needs_redraw = true;
            }

            let idle_summary_due = {
                let mut state = app_state.lock().await;
                let background_tasks_active =
                    crate::tools::has_background_tasks(&state.active_session_id);
                let due = state.should_start_idle_summary(
                    std::time::Instant::now(),
                    background_tasks_active,
                    IDLE_SUMMARY_AFTER,
                );
                if due { state.claim_summary() } else { false }
            };
            if idle_summary_due {
                let state_clone = std::sync::Arc::clone(&app_state);
                let client_clone = client.clone();
                tokio::spawn(async move {
                    crate::app::summarize_session_after_idle(&state_clone, &client_clone).await;
                });
            }

            task_subscriptions.retain(|session_id, _| {
                session_id == &active_session_id || manager.has_running(session_id)
            });
            for session_id in disconnected_sessions {
                task_subscriptions.remove(&session_id);
            }
            for session_id in late_disconnected_sessions {
                task_subscriptions.remove(&session_id);
            }

            {
                let mut state = app_state.lock().await;
                if state.expire_ctrl_c_exit_arming(std::time::Instant::now()) {
                    needs_redraw = true;
                }
            }

            let update_version = {
                let mut state = app_state.lock().await;
                if state.update_requested {
                    state.update_requested = false;
                    match state.update_check {
                        crate::update::UpdateState::Available(latest) => Some(Some(latest)),
                        _ => Some(None),
                    }
                } else {
                    None
                }
            };
            if let Some(target) = update_version {
                let target_version = match target {
                    Some(v) => Some(v),
                    None => match crate::update::check_for_update(&client).await {
                        Ok(crate::update::UpdateCheck::Available { latest, .. }) => Some(latest),
                        Ok(crate::update::UpdateCheck::UpToDate { current, latest }) => {
                            let mut state = app_state.lock().await;
                            state.update_check = crate::update::UpdateState::UpToDate(latest);
                            state.set_notice(format!(
                                "✨ RustCode v{} is up to date (latest: v{}).",
                                crate::update::format_version(current),
                                crate::update::format_version(latest)
                            ));
                            needs_redraw = true;
                            None
                        }
                        Err(error) => {
                            let mut state = app_state.lock().await;
                            state.update_check = crate::update::UpdateState::Failed;
                            state.set_warning_notice(format!("Update check failed: {error}"));
                            needs_redraw = true;
                            None
                        }
                    },
                };
                if let Some(latest) = target_version {
                    match run_update_command(&mut terminal_runtime, &client, latest).await {
                        Ok(()) => println!("🎉 Update ran successfully! Please restart rustcode."),
                        Err(error) => eprintln!("Update failed: {error}"),
                    }
                    update_exit = true;
                    break;
                }
                continue;
            }

            // Ratatui's inline viewport grows/shrinks by appending and clearing
            // terminal rows. When the terminal is resized, update the viewport
            // bounds and clear the live area so the active frame redraws cleanly.
            if handle_terminal_resize(
                &mut terminal_runtime,
                &app_state,
                &mut terminal_size,
                &mut transcript_cursor,
                &mut transcript_state,
                &mut stream_commits,
                &mut replaying_transcript,
            )
            .await?
            {
                needs_redraw = true;
            }

            let (response_active, background_redraw) = {
                let mut s = app_state.lock().await;
                let background_active = crate::tools::has_background_tasks(&s.active_session_id);
                (
                    s.status_state().is_active() || background_active,
                    s.take_redraw_request(),
                )
            };
            needs_redraw |= background_redraw;
            while let Ok(agent_event) = agent_ui_event_receiver.try_recv() {
                if matches!(&agent_event, AgentUiEvent::ApprovalRequested { .. }) {
                    let _ = app_event_sender.send(AppEvent::OpenOverlay(
                        crate::app::events::Overlay::ToolConfirmation,
                    ));
                }
                transcript_state.apply_agent_event(&agent_event);
                frame_requester.schedule_frame();
                needs_redraw = true;
            }
            if response_active {
                frame_requester.schedule_frame();
            }

            {
                let mut s = app_state.lock().await;
                if !s.summary_in_flight && !s.orchestrator_running && !s.pending_queue.is_empty() {
                    s.orchestrator_running = true;
                    s.status = AppStatus::Queued;
                    let client_clone = client.clone();
                    let state_clone = Arc::clone(&app_state);
                    let token_clone = current_cancel_token.clone();
                    let ui_event_sender = agent_ui_event_sender.clone();
                    drop(s);
                    tokio::spawn(async move {
                        crate::network::process_queue_orchestrator_with_ui_events(
                            client_clone,
                            state_clone,
                            token_clone,
                            std::sync::Arc::new(crate::network::policy::InteractivePolicy),
                            ui_event_sender,
                        )
                        .await;
                    });
                    needs_redraw = true;
                }
            }

            let response_just_finished = was_responding && !response_active;
            if crate::app::status::should_notify_response_finished(
                response_just_finished,
                terminal_focused,
            ) {
                notify_response_finished(&mut terminal_runtime);
            }
            was_responding = response_active;
            let should_draw = needs_redraw || frame_stream.try_next().is_some();

            if should_draw {
                render_frame(RenderFrameContext {
                    terminal_runtime: &mut terminal_runtime,
                    app_state: &app_state,
                    transcript_cursor: &mut transcript_cursor,
                    transcript_state: &mut transcript_state,
                    stream_commits: &mut stream_commits,
                    replaying_transcript: &mut replaying_transcript,
                    response_active,
                    response_just_finished,
                    last_progress_sent: &mut last_progress_sent,
                })
                .await?;
                needs_redraw = false;
            }

            if let Ok(event_result) =
                tokio::time::timeout(EVENT_POLL_INTERVAL, tui_events.next()).await
            {
                let Some(ev) = event_result? else {
                    continue;
                };
                let _ = app_event_sender.send(AppEvent::Tui(ev));
            }

            let Some(app_event) = app_event_receiver.try_recv().ok() else {
                continue;
            };
            match handle_app_event(
                app_event,
                InputContext {
                    terminal_runtime: &mut terminal_runtime,
                    app_state: &app_state,
                    client: &client,
                    current_cancel_token: &mut current_cancel_token,
                    needs_redraw: &mut needs_redraw,
                    terminal_focused: &mut terminal_focused,
                    transcript_state: &mut transcript_state,
                    app_event_sender: &app_event_sender,
                    composer: &composer,
                },
            )
            .await?
            {
                InputFlow::ContinueIteration => continue,
                InputFlow::ContinueLoop => {}
                InputFlow::Exit { update } => {
                    update_exit = update;
                    break;
                }
            }
        }

        let mut exit_summary = {
            let s = app_state.lock().await;
            s.subagent_supervisor.shutdown();
            crate::ExitSummary::from_state(&s)
        };
        if update_exit {
            exit_summary.print_handoff = false;
        }
        crate::config::flush_history();
        restore_terminal(&mut terminal_runtime, exit_summary.composer_y)?;
        Ok(exit_summary)
    }
}

#[cfg(test)]
mod tests {
    use super::record_active_background_task;
    use crate::app::AppState;
    use crate::tools::ToolExecutionOutput;

    #[test]
    fn active_completion_is_recorded_and_queued_once() {
        let mut state = AppState::new();
        state.active_session_id = "active-completion-session".to_owned();
        let output = ToolExecutionOutput::success("cargo test passed".to_owned());

        assert!(record_active_background_task(
            &mut state,
            "task-completed-once",
            output.clone()
        ));
        assert!(!record_active_background_task(
            &mut state,
            "task-completed-once",
            output
        ));
        assert_eq!(state.history.len(), 1);
        assert_eq!(
            state.pending_queue,
            vec!["__task_wakeup__:task-completed-once".to_owned()]
        );
        assert!(state.background_wakeup_ids.contains("task-completed-once"));
    }

    #[test]
    fn session_subscription_retains_inactive_completion_until_consumed() {
        let manager = rustcode_tasks::TaskManager::new(std::sync::Arc::new(|_| true));
        let subscription = manager.subscribe_session("inactive-session");
        let task = manager
            .spawn_with_id(
                "inactive-completion-task",
                rustcode_tasks::TaskSpec::new(
                    "inactive-session",
                    rustcode_command::CommandRequest {
                        command: if cfg!(target_os = "windows") {
                            "echo retained".to_owned()
                        } else {
                            "printf retained".to_owned()
                        },
                        cwd: None,
                        env: Vec::new(),
                        timeout: std::time::Duration::from_secs(5),
                        process_group: true,
                    },
                ),
            )
            .expect("spawn inactive task");

        let mut saw_finished = false;
        while let Ok(event) = subscription.recv() {
            if event.task_id() == task.id() && event.is_terminal() {
                saw_finished = true;
                break;
            }
        }
        assert!(saw_finished, "inactive session completion was retained");
    }
}
