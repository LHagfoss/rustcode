use super::*;
use std::sync::mpsc::TryRecvError;

const IDLE_SUMMARY_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

impl AppRuntime {
    pub(crate) async fn run(self) -> Result<crate::run::ExitSummary, Box<dyn Error>> {
        let AppRuntime {
            terminal_runtime,
            app_state,
            discord_rpc,
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
                    rustcode::tools::background_task_manager()
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
            let manager = rustcode::tools::background_task_manager();
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
                    rustcode::tools::has_background_tasks(&state.active_session_id);
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
                    rustcode::app::summarize_session_after_idle(&state_clone, &client_clone).await;
                });
            }

            // Stall watchdog (issue #1226): the main loop stays alive on
            // stdin even when the turn machinery died mid-stream, freezing
            // the TUI forever with zero evidence. If status claims active
            // work but nothing can progress, log it and reset to Idle so
            // the user can reissue instead of force-quitting.
            let stall_recovered = {
                let mut state = app_state.lock().await;
                let background_tasks_active =
                    rustcode::tools::has_background_tasks(&state.active_session_id);
                match state.check_stall_watchdog(background_tasks_active, std::time::Instant::now())
                {
                    None => None,
                    Some(recovery) => {
                        if recovery.reset_orchestrator {
                            state.invalidate_orchestrator();
                        }
                        let notice = if recovery.queue_preserved {
                            "[Watchdog: queued work stalled with no active turn for 5 minutes; state reset and queued prompts restarted. Reissue anything still missing.]"
                        } else {
                            "[Watchdog: turn showed no progress for 5 minutes with nothing running; state reset to Idle. Reissue the request if work is still needed.]"
                        };
                        state
                            .history
                            .push(rustcode::app::ChatMessage::new("system", notice));
                        let session_id = state.active_session_id.clone();
                        rustcode::config::save_session_history(&session_id, &state.history);
                        state.clear_active_turn_projection();
                        state.enter_idle();
                        state.request_redraw();
                        Some(recovery.queue_preserved)
                    }
                }
            };
            if let Some(queue_preserved) = stall_recovered {
                rustcode::logger::operational_event(
                    "turn.stall_recovered",
                    serde_json::json!({ "queue_preserved": queue_preserved }),
                );
                needs_redraw = true;
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
                let now = std::time::Instant::now();
                if state.expire_ctrl_c_exit_arming(now) {
                    needs_redraw = true;
                }
                if state.refresh_workspace_location(now) {
                    needs_redraw = true;
                }
            }

            let update_version = {
                let mut state = app_state.lock().await;
                if state.update_requested {
                    state.update_requested = false;
                    match state.update_check {
                        rustcode_core::update::UpdateState::Available(latest) => Some(Some(latest)),
                        _ => Some(None),
                    }
                } else {
                    None
                }
            };
            if let Some(target) = update_version {
                let target_version = match target {
                    Some(v) => Some(v),
                    None => match rustcode::update::check_for_update(&client).await {
                        Ok(rustcode_core::update::UpdateCheck::Available { latest, .. }) => {
                            Some(latest)
                        }
                        Ok(rustcode_core::update::UpdateCheck::UpToDate { current, latest }) => {
                            let mut state = app_state.lock().await;
                            state.update_check =
                                rustcode_core::update::UpdateState::UpToDate(latest);
                            state.set_notice(format!(
                                "✨ RustCode v{} is up to date (latest: v{}).",
                                rustcode_core::update::format_version(current),
                                rustcode_core::update::format_version(latest)
                            ));
                            needs_redraw = true;
                            None
                        }
                        Err(error) => {
                            let mut state = app_state.lock().await;
                            state.update_check = rustcode_core::update::UpdateState::Failed;
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
                let background_active = rustcode::tools::has_background_tasks(&s.active_session_id);
                (
                    s.status_state().is_active() || background_active,
                    s.take_redraw_request(),
                )
            };
            needs_redraw |= background_redraw;
            while let Ok(agent_event) = agent_ui_event_receiver.try_recv() {
                if matches!(&agent_event, AgentUiEvent::ApprovalRequested { .. }) {
                    let _ = app_event_sender.send(AppEvent::OpenOverlay(
                        rustcode::app::events::Overlay::ToolConfirmation,
                    ));
                }
                transcript_state.apply_agent_event(&agent_event);
                frame_requester.schedule_frame();
                needs_redraw = true;
            }
            if response_active {
                frame_requester.schedule_frame();
            }

            let should_drain_queue = {
                let state = app_state.lock().await;
                !state.summary_in_flight
                    && !state.orchestrator_running
                    && !state.pending_queue.is_empty()
            };
            if should_drain_queue
                && spawn_observed_orchestrator(
                    client.clone(),
                    Arc::clone(&app_state),
                    current_cancel_token.clone(),
                    agent_ui_event_sender.clone(),
                )
                .await
            {
                needs_redraw = true;
            }

            let response_just_finished = was_responding && !response_active;
            if rustcode::app::status::should_notify_response_finished(
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
                    discord_rpc: &discord_rpc,
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
                    agent_ui_event_sender: &agent_ui_event_sender,
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
            crate::run::ExitSummary::from_state(&s)
        };
        if update_exit {
            exit_summary.print_handoff = false;
        }
        rustcode::config::flush_history();
        restore_terminal(&mut terminal_runtime, exit_summary.composer_y)?;
        discord_rpc.shutdown();
        Ok(exit_summary)
    }
}
