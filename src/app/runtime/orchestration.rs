use super::*;

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
        let update_exit;
        let mut last_progress_sent = std::time::Instant::now();
        let composer = ui::Composer::new();
        loop {
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
                if !s.orchestrator_running && !s.pending_queue.is_empty() {
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
