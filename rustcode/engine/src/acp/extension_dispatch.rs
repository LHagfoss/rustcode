use super::*;

pub(super) async fn dispatch_request(
    server: &AcpServer,
    client_id: clients::ClientId,
    message: agent_client_protocol::UntypedMessage,
    responder: agent_client_protocol::Responder,
    connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    unhandled: &mut Option<agent_client_protocol::Dispatch>,
) -> Result<(), agent_client_protocol::Error> {
    let (method, params) = message.into_parts();
    match method.as_str() {
        "_rustcode/threads/delete" => {
            let server = server.clone();
            let request: ThreadDeleteRequest = parse_params(params)?;

            server.announce_thread_deleted(ThreadDeletedNotification {
                session_id: request.session_id,
            });
            responder.respond(serde_json::json!({}))?;
            Ok(())
        }
        "_rustcode/presence" => {
            let server = server.clone();
            let request: presence::PresenceRequest = parse_params(params)?;

            responder.respond(server.presence.status(request.cwd))
        }
        "_rustcode/presence/set" => {
            let server = server.clone();
            let request: presence::SetPresenceRequest = parse_params(params)?;

            responder.respond(server.presence.set_enabled(request)?)
        }
        "_rustcode/settings/get" => {
            let request: settings::GetSettingsRequest = parse_params(params)?;

            // A client that reads settings follows a change made elsewhere.
            server
                .clients
                .subscribe(client_id, clients::Topic::Settings);
            settings::start_watching(&server.clients);
            responder.respond(settings::read(request)?)
        }
        "_rustcode/settings/set" => {
            let request: settings::SetSettingsRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::Settings);
            settings::start_watching(&server.clients);
            responder.respond(settings::write(request)?)
        }
        "_rustcode/rewind" => {
            let server = server.clone();
            let request: rewind::RewindRequest = parse_params(params)?;

            let Some(agent) = server.builtin_agent(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            let answer = agent
                .rewind(request.user_index, &request.text, request.first_only)
                .await;
            match answer {
                Ok(answer) => {
                    // No update says the chat was cut: nobody
                    // resumes from before it.
                    server.clients.invalidate_cursors(&request.session_id);
                    responder.respond(answer)
                }
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/move" => {
            let server = server.clone();
            let request: session_move::MoveSessionRequest = parse_params(params)?;

            if !valid_acp_session_id(&request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data("invalid session id"),
                );
            }
            let cwd = match canonical_acp_cwd(&request.cwd, "session/move") {
                Ok(cwd) => cwd,
                Err(error) => return responder.respond_with_error(error),
            };
            // An open chat has a turn, tools and tasks bound to its folder.
            if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params()
                        .data("close the session before moving it"),
                );
            }
            match session_move::move_session(&request.session_id, cwd) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/replay" => {
            let request: session_replay::ReplaySessionRequest = parse_params(params)?;

            match session_replay::replay_session(&request.session_id) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/export" => {
            let server = server.clone();
            let request: session_transfer::ExportSessionRequest = parse_params(params)?;

            match session_transfer::export_session(&server.external, request).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/import" => {
            let server = server.clone();
            let request: session_transfer::ImportSessionRequest = parse_params(params)?;

            match session_transfer::import_session(&server, request).await {
                Ok((answer, imported)) => {
                    if let Some(imported) = imported {
                        server.announce_thread_created(ThreadCreatedNotification {
                            meta: None,
                            session_id: imported.session_id,
                            cwd: imported.cwd,
                            title: imported.title,
                            harness: Some(imported.harness),
                            model: imported.model,
                        });
                    }
                    responder.respond(answer)
                }
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/remove" => {
            let server = server.clone();
            let request: session_transfer::RemoveSessionRequest = parse_params(params)?;

            match session_transfer::remove_session(&server, request).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/session/repair" => {
            let server = server.clone();
            let request: session_repair::RepairSessionRequest = parse_params(params)?;

            if !valid_acp_session_id(&request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data("invalid session id"),
                );
            }
            if session_id_is_active(&*server.sessions.lock().await, &request.session_id) {
                return responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params()
                        .data("close the session before repairing it"),
                );
            }
            match session_repair::repair_session(&request.session_id) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        "_rustcode/usage/plans" => {
            let request: usage::UsagePlansRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(usage::plans(request).await)?;
                Ok(())
            })
        }
        "_rustcode/usage/shared" => {
            let request: usage::UsageSharedRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(usage::shared_plans(request).await)?;
                Ok(())
            })
        }
        "_rustcode/usage/limits" => {
            let server = server.clone();
            let request: usage::UsageLimitsRequest = parse_params(params)?;

            let Some(agent) = server.builtin_agent(&request.session_id).await else {
                // A thread on another harness has no engine session: its plan is that
                // harness's own (the Codex CLI's windows for Codex).
                let Some(harness) = usage::external_harness(&request.session_id) else {
                    return responder.respond_with_error(unknown_session(&request.session_id));
                };
                let session_id = request.session_id.clone();
                return connection.spawn(async move {
                    // A named account's own login, so the meter never
                    // shows another home's numbers.
                    let home = usage::account_home(&session_id, &harness);
                    let model = usage::external_model(&session_id);
                    responder.respond(
                        usage::harness_limits(&harness, home.as_deref(), model.as_deref()).await,
                    )?;
                    Ok(())
                });
            };
            connection.spawn(async move {
                responder.respond(agent.usage_limits().await)?;
                Ok(())
            })
        }
        "_rustcode/commands" => {
            let _request: commands::CommandsRequest = parse_params(params)?;

            connection.spawn(async move {
                match commands::catalog_update() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/signin" => {
            let request: outside::OutsideSignInRequest = parse_params(params)?;

            match outside::sign_in(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/passkey/add" => {
            let request: outside::OutsidePasskeyAddRequest = parse_params(params)?;

            match outside::passkey_add(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/passkey/remove" => {
            let request: outside::OutsidePasskeyRemoveRequest = parse_params(params)?;

            match outside::passkey_remove(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/web/sign-out" => {
            let request: outside::OutsideWebSignOutRequest = parse_params(params)?;

            match outside::web_sign_out(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/code" => {
            let request: outside::OutsideCodeRequest = parse_params(params)?;

            match outside::code(request) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/outside/list" => {
            let request: outside::OutsideListRequest = parse_params(params)?;

            // A client that lists outside agents hears when one waits for it.
            server.clients.subscribe(client_id, clients::Topic::Outside);
            outside::start_publishing(&server.clients);
            connection.spawn(async move {
                match outside::list(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/create" => {
            let request: outside::OutsideCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::create(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/update" => {
            let request: outside::OutsideUpdateRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::update(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/revoke" => {
            let request: outside::OutsideRevokeRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::revoke(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/outside/answer" => {
            let request: outside::OutsideAnswerRequest = parse_params(params)?;

            connection.spawn(async move {
                match outside::answer(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/usage" => {
            let request: data::DataUsageRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::usage(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/query" => {
            let request: data::DataQueryRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::query(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/activity" => {
            let request: data::DataActivityRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::activity(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/links/load" => {
            let _request: data::DataLinksLoadRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::links_load() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/links/save" => {
            let request: data::DataLinksSaveRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::links_save(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/receipts/get" => {
            let request: data::DataReceiptsGetRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::receipts_get(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/relocate" => {
            let server = server.clone();
            let request: project_relocate::RelocateRequest = parse_params(params)?;

            // Hold both registries through validation and writes: a closed thread
            // cannot be loaded between the check and its path being replaced.
            let sessions = server.sessions.lock().await;
            let external = server.external.lock().await;
            match project_relocate::relocate(request, |id| {
                sessions.contains_key(id) || external.get(id).is_some_and(|s| !s.is_closed())
            }) {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder
                    .respond_with_error(agent_client_protocol::Error::invalid_params().data(error)),
            }
        }
        "_rustcode/machine/exec" => {
            let request: machine_exec::MachineExecRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_exec::exec(request, tokio_util::sync::CancellationToken::new()).await
                {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/machine/update" => {
            let request: machine_update::MachineUpdateRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_update::handle(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/read" => {
            let request: projects::ProjectReadRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::read(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/browse" => {
            let request: projects::ProjectBrowseRequest = parse_params(params)?;

            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || projects::browse(request)).await {
                    Ok(Ok(answer)) => responder.respond(answer),
                    Ok(Err(error)) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/create" => {
            let request: projects::ProjectCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::create(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/project/clone" => {
            let request: projects::ProjectCloneRequest = parse_params(params)?;

            connection.spawn(async move {
                match projects::clone(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/attachments/read" => {
            let request: attachments::AttachmentReadRequest = parse_params(params)?;

            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || attachments::read(request)).await {
                    Ok(Ok(answer)) => responder.respond(answer),
                    Ok(Err(error)) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/attachments/upload" => {
            let request: attachments::AttachmentUploadRequest = parse_params(params)?;

            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || attachments::upload(request)).await {
                    Ok(Ok(answer)) => responder.respond(answer),
                    Ok(Err(error)) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/environment/sync" => {
            let request: machine_sync::MachineSyncRequest = parse_params(params)?;

            connection.spawn(async move {
                match machine_sync::sync(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/list" => {
            let request: loops::LoopsListRequest = parse_params(params)?;

            connection.spawn(async move {
                match {
                    let _ = request;
                    loops::list().await
                } {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/save" => {
            let request: loops::LoopsSaveRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::save(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/set_paused" => {
            let request: loops::LoopsSetPausedRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::set_paused(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/run" => {
            let request: loops::LoopsRunRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::run(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/cancel" => {
            let request: loops::LoopsCancelRequest = parse_params(params)?;

            connection.spawn(async move {
                match loops::cancel(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/delete" => {
            let request: loops::LoopsDeleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::delete(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/loops/start_daemon" => {
            let request: loops::LoopsStartDaemonRequest = parse_params(params)?;

            connection.spawn(async move {
                match {
                    let _ = request;
                    loops::start_daemon().await
                } {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/list" => {
            let _request: scheduled::ScheduledListRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::list().await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/create" => {
            let request: scheduled::ScheduledCreateRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::create(request).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/delete" => {
            let request: scheduled::ScheduledDeleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::delete(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/scheduled/run" => {
            let request: scheduled::ScheduledRunRequest = parse_params(params)?;

            connection.spawn(async move {
                match scheduled::run(request.id).await {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/mirror" => {
            let request: mirror::MirrorRequest = parse_params(params)?;

            connection.spawn(async move {
                // The mirror reads and writes files; keep that off the connection's task.
                let answer = tokio::task::spawn_blocking(move || mirror::answer(request))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));
                match answer {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/config/telemetry" => {
            let _request: telemetry::TelemetryRequest = parse_params(params)?;

            connection.spawn(async move {
                match telemetry::answer() {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/data/receipts/put" => {
            let request: data::DataReceiptsPutRequest = parse_params(params)?;

            connection.spawn(async move {
                match data::receipts_put(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/config/telemetry/set" => {
            let request: telemetry::SetTelemetryRequest = parse_params(params)?;

            connection.spawn(async move {
                match telemetry::answer_set(request) {
                    Ok(answer) => responder.respond(answer),
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error),
                    ),
                }?;
                Ok(())
            })
        }
        "_rustcode/agents" => {
            let server = server.clone();
            let request: agents::AgentsRequest = parse_params(params)?;

            let Some((agent, cwd)) = server.builtin_agent_and_cwd(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            match agent.agents(&cwd, &server.client, request.action).await {
                Ok(answer) => responder.respond(answer),
                Err(error) => responder.respond_with_error(error),
            }
        }
        // A tool call to an MCP server waits for the user's approval, so it
        // runs off the dispatch loop.
        "_rustcode/mcp/app" => {
            let server = server.clone();
            let request: mcp_app::McpAppRequest = parse_params(params)?;

            let Some(agent) = server.builtin_agent(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            let builtin = harnesses::get(&server.external, &request.session_id)
                .await
                .is_none();
            let clients = server.clients.session(&request.session_id);
            let auto_approve = server.auto_approve;
            connection.spawn(async move {
                match agent.mcp_app(request, builtin, clients, auto_approve).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/steer" => {
            let server = server.clone();
            let request: steer::SteerRequest = parse_params(params)?;

            if let Some(external) = harnesses::get(&server.external, &request.session_id).await {
                let remote = server.remote.clone();
                return connection.spawn(async move {
                    let mut blocks = request.prompt.clone().unwrap_or_else(|| {
                        vec![agent_client_protocol::schema::v1::ContentBlock::Text(
                            agent_client_protocol::schema::v1::TextContent::new(
                                request.text.clone(),
                            ),
                        )]
                    });
                    if let Err(error) = crate::controller::materialize_images(&mut blocks) {
                        responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params().data(error),
                        )?;
                        return Ok(());
                    }
                    let accepted = external.steer(&blocks).await;
                    if accepted {
                        remote.note_prompt(
                            &request.session_id,
                            &request.text,
                            request
                                .meta
                                .as_ref()
                                .and_then(|v| v.get("rustcode"))
                                .and_then(|v| v.get("origin"))
                                .cloned()
                                .filter(|v| crate::message_origin::from_meta(v).is_some()),
                        );
                    }
                    responder.respond(serde_json::json!({ "accepted": accepted }))?;
                    Ok(())
                });
            }
            let Some(agent) = server.builtin_agent(&request.session_id).await else {
                return responder.respond_with_error(unknown_session(&request.session_id));
            };
            // The built-in harness reads a steer as text; one with images waits for the next turn.
            let images = request.prompt.iter().flatten().any(|block| {
                matches!(
                    block,
                    agent_client_protocol::schema::v1::ContentBlock::Image(_)
                )
            });
            if images {
                return responder.respond(serde_json::json!({ "accepted": false }));
            }
            let origin = request
                .meta
                .as_ref()
                .and_then(|v| v.get("rustcode"))
                .and_then(|v| v.get("origin"))
                .cloned()
                .filter(|v| crate::message_origin::from_meta(v).is_some());
            responder.respond(agent.steer(&request.text, origin).await)
        }
        // Remote access starts a gateway, waits for a pairing answer and runs
        // launchctl: keep the dispatch loop free while each does.
        "_rustcode/remote" => {
            let server = server.clone();
            let request: remote::RemoteStatusRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.status(&request.session_id).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/start" => {
            let server = server.clone();
            let request: remote::RemoteStartRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                if request.session_id.is_empty() {
                    // Settings › Remote access: switch the machine on
                    // with no thread named. Every open thread shares
                    // itself from the next auto-share pass.
                    return responder.respond(server.remote.enable().await);
                }
                let session = {
                    let sessions = server.sessions.lock().await;
                    sessions.get(&request.session_id).map(|session| {
                        remote::RemoteSession::from_acp(
                            request.session_id.clone(),
                            session,
                            server.clients.session(&request.session_id),
                            Arc::clone(&server.client),
                            server.auto_approve,
                            server.presence.clone(),
                            server.remote.clone(),
                        )
                    })
                };
                let Some(session) = session else {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data(format!("unknown ACP session: {}", request.session_id)),
                    );
                };
                match server.remote.start(session, request.pair).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error.into_acp())?,
                }
                Ok(())
            })
        }
        "_rustcode/remote/pair" => {
            let server = server.clone();
            let request: remote::RemotePairRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.pair(&request.session_id).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/stop" => {
            let server = server.clone();
            let request: remote::RemoteStopRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                match server.remote.stop(&request.session_id).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/remote/answer" => {
            let server = server.clone();
            let request: remote::RemoteAnswerRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(
                    server
                        .remote
                        .answer_connect(&request.id, request.accept)
                        .await,
                )?;
                Ok(())
            })
        }
        "_rustcode/remote/connect_code" => {
            let request: remote::RemoteConnectCodeRequest = parse_params(params)?;

            connection.spawn(async move {
                responder.respond(remote::connect_code(request.cancel).await)?;
                Ok(())
            })
        }
        "_rustcode/remote/revoke" => {
            let server = server.clone();
            let request: remote::RemoteRevokeRequest = parse_params(params)?;

            let server = server.clone();
            connection.spawn(async move {
                responder.respond(server.remote.revoke(&request.device_id).await)?;
                Ok(())
            })
        }
        // Harness detection runs commands and a model listing may start
        // one, so none of these holds the dispatch loop.
        "_rustcode/harnesses" => {
            let _request: harnesses::HarnessesRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::snapshot().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/models" => {
            let request: harnesses::HarnessModelsRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::models(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/set" => {
            let request: harnesses::SetHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::set(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/remove" => {
            let request: harnesses::RemoveHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::remove(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/default" => {
            let request: harnesses::DefaultHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::set_default(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/remember" => {
            let request: harnesses::RememberHarnessRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::remember(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/add" => {
            let request: harnesses::AddHarnessAccountRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::add_account(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/remove" => {
            let server = server.clone();
            let request: harnesses::RemoveHarnessAccountRequest = parse_params(params)?;

            let external = server.external.clone();
            connection.spawn(async move {
                match harnesses::remove_account(&external, request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/harnesses/accounts/check" => {
            let request: harnesses::CheckHarnessAccountsRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::check_accounts(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/session/harness" => {
            let server = server.clone();
            let request: harnesses::SwitchHarnessRequest = parse_params(params)?;

            let server = server.clone();
            let peer = server.clients.peer(client_id, &connection);
            connection.spawn(async move {
                let session_id = request.session_id.clone();
                let builtin = if harnesses::get(&server.external, &session_id)
                    .await
                    .is_some()
                {
                    None
                } else if crate::thread_status::is_running(&session_id) {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data("stop the running turn before switching harness"),
                    );
                } else {
                    close_builtin(&server, &session_id).await
                };
                match harnesses::switch(&server.external, request, builtin, &peer, &server.remote)
                    .await
                {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/onboarding/complete" => {
            let _request: harnesses::OnboardingCompleteRequest = parse_params(params)?;

            connection.spawn(async move {
                match harnesses::complete_onboarding().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities" => {
            let _request: capabilities::CapabilitiesRequest = parse_params(params)?;

            responder.respond(capabilities::snapshot_value()?)
        }
        "_rustcode/capabilities/set" => {
            let request: capabilities::SetCapabilityRequest = parse_params(params)?;

            // Starting an MCP server can take seconds; keep the dispatch
            // loop free while it does.
            connection.spawn(async move {
                match capabilities::set_capability(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/add" => {
            let request: capabilities::AddMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::add_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/remove" => {
            let request: capabilities::RemoveMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::remove_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/restart" => {
            let request: capabilities::RestartMcpServerRequest = parse_params(params)?;

            // Connecting a server again can take the same seconds a start
            // does; keep the dispatch loop free while it does.
            connection.spawn(async move {
                match capabilities::restart_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/capabilities/mcp/logout" => {
            let request: capabilities::LogoutMcpServerRequest = parse_params(params)?;

            connection.spawn(async move {
                match capabilities::logout_mcp_server(request).await {
                    Ok(snapshot) => responder.respond(snapshot)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/skills/add" => {
            let request: capabilities::AddSkillRequest = parse_params(params)?;

            responder.respond(capabilities::add_skill(request)?)
        }
        "_rustcode/skills/get" => {
            let request: capabilities::GetSkillRequest = parse_params(params)?;

            responder.respond(capabilities::get_skill(request)?)
        }
        // Orchestrator v2 (`docs/orchestrator.md`): worktrees, thread status
        // and checkpoints. Each runs git, so none holds the dispatch loop.
        "_rustcode/worktrees/create" => {
            let request: crate::worktrees::CreateRequest = parse_params(params)?;

            connection.spawn(async move {
                let created =
                    tokio::task::spawn_blocking(move || crate::worktrees::create(&request)).await;
                match created {
                    Ok(Ok(worktree)) => responder.respond(serde_json::json!(worktree))?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/list" => {
            let request: crate::worktrees::ListRequest = parse_params(params)?;

            connection.spawn(async move {
                let listed =
                    tokio::task::spawn_blocking(move || crate::worktrees::list(&request.repo))
                        .await;
                match listed {
                    Ok(Ok(worktrees)) => {
                        responder.respond(serde_json::json!({ "worktrees": worktrees }))?
                    }
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/remove" => {
            let request: crate::worktrees::RemoveRequest = parse_params(params)?;

            connection.spawn(async move {
                let removed = tokio::task::spawn_blocking(move || {
                    crate::worktrees::remove(&request.path, request.force)
                })
                .await;
                match removed {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/textGeneration/configure" => {
            let request: crate::text_generation::ConfigureRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::text_generation::configure(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/voice/transcribe" => {
            let request: crate::transcription::TranscribeRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::transcription::transcribe(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/textGeneration/generate" => {
            let request: crate::text_generation::GenerateRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::text_generation::generate(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/git/state" => {
            let request: crate::git_actions::StateRequest = parse_params(params)?;

            connection.spawn(async move {
                let state =
                    tokio::task::spawn_blocking(move || crate::git_actions::state(&request.path))
                        .await;
                match state {
                    Ok(Ok(state)) => responder.respond(state)?,
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/git/run" => {
            let request: crate::git_actions::RunRequest = parse_params(params)?;

            connection.spawn(async move {
                match crate::git_actions::run(request).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(error.into_acp())?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/state" => {
            let request: crate::worktrees::StateRequest = parse_params(params)?;

            connection.spawn(async move {
                let states =
                    tokio::task::spawn_blocking(move || crate::worktrees::states(&request.paths))
                        .await;
                match states {
                    Ok(rows) => responder.respond(serde_json::json!({ "worktrees": rows }))?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/worktrees/handoff" => {
            let request: crate::worktrees::HandoffRequest = parse_params(params)?;

            connection.spawn(async move {
                    let to = crate::worktrees::Handoff::parse(request.to.trim());
                    let Some(to) = to else {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data(serde_json::json!({
                                    "reason": "invalid",
                                    "message": format!("to must be checkout or worktree, not '{}'", request.to),
                                })),
                        );
                    };
                    let moved = tokio::task::spawn_blocking(move || {
                        crate::worktrees::handoff(
                            &request.session_id,
                            to,
                            request.title.as_deref(),
                            request.branch.as_deref(),
                        )
                    })
                    .await;
                    match moved {
                        Ok(Ok(answer)) => responder.respond(answer)?,
                        Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                        Err(error) => responder.respond_with_error(agent_client_protocol::Error::internal_error().data(error.to_string()))?,
                    }
                    Ok(())
                })
        }
        "_rustcode/session/pin" => {
            let request: crate::thread_status::PinRequest = parse_params(params)?;

            match crate::thread_status::set_pinned(&request.session_id, request.pinned) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/permissions" => {
            let _request: permissions::PermissionsRequest = parse_params(params)?;

            // The user config is what every request's policy reads, so a
            // client lists and revokes there rather than in one session.
            let (_, _, config) = crate::config::load_config();
            responder.respond(permissions::grants_snapshot(&config))
        }
        "_rustcode/permissions/revoke" => {
            let server = server.clone();
            let request: permissions::RevokePermissionRequest = parse_params(params)?;

            let (_, _, mut config) = crate::config::load_config();
            let answer = permissions::revoke_grant(&mut config, &request.id);
            // Open sessions hold their own copy of the user config: the
            // one that answers a call must stop covering it now.
            for session in server.sessions.lock().await.values() {
                session
                    .agent
                    .set_timed_command_approvals(config.timed_command_approvals.clone())
                    .await;
            }
            responder.respond(answer)
        }
        "_rustcode/themes" => {
            let request: themes::ThemesRequest = parse_params(params)?;

            // A client that lists themes restyles when they change.
            server.clients.subscribe(client_id, clients::Topic::Themes);
            themes::start_watching(&server.clients);
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || themes::answer(request)).await {
                    Ok(answer) => responder.respond(answer)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/themes/set" => {
            let request: themes::SetThemeRequest = parse_params(params)?;

            server.clients.subscribe(client_id, clients::Topic::Themes);
            themes::start_watching(&server.clients);
            connection.spawn(async move {
                match tokio::task::spawn_blocking(move || themes::answer_set(request)).await {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/session/snooze" => {
            let request: crate::thread_status::SnoozeRequest = parse_params(params)?;

            match crate::thread_status::set_snoozed(&request.session_id, request.until) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/session/title" => {
            let request: crate::thread_status::SetTitleRequest = parse_params(params)?;

            match crate::thread_status::set_title(&request) {
                Ok(answer) => responder.respond(answer),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/session/status" => {
            let request: crate::thread_status::SetStatusRequest = parse_params(params)?;

            match crate::thread_status::set(&request.session_id, &request.status) {
                Ok(()) => responder.respond(serde_json::json!({})),
                Err(message) => responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params().data(message),
                ),
            }
        }
        "_rustcode/threads/search" => {
            let request: crate::thread_index::SearchRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer = tokio::task::spawn_blocking(move || {
                    crate::thread_index::answer_search(&request)
                })
                .await;
                match answer {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(message)) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(message),
                    )?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/checkpoints/list" => {
            let request: crate::checkpoints::ListRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer =
                    tokio::task::spawn_blocking(move || crate::checkpoints::answer_list(&request))
                        .await;
                match answer {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/checkpoints/diff" => {
            let request: crate::checkpoints::DiffRequest = parse_params(params)?;

            connection.spawn(async move {
                let answer =
                    tokio::task::spawn_blocking(move || crate::checkpoints::answer_diff(&request))
                        .await;
                match answer {
                    Ok(Ok(answer)) => responder.respond(answer)?,
                    Ok(Err(error)) => responder.respond_with_error(error)?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/checkpoints/restore" => {
            let server = server.clone();
            let request: crate::checkpoints::RestoreRequest = parse_params(params)?;

            // Only an open built-in thread rewinds its conversation.
            let agent = server.builtin_agent(&request.session_id).await;
            let clients = server.clients.clone();
            connection.spawn(async move {
                match crate::checkpoints::answer_restore(&request, agent).await {
                    Ok(answer) => {
                        // A restore may cut the chat without an update.
                        clients.invalidate_cursors(&request.session_id);
                        responder.respond(answer)?
                    }
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/server/status" => {
            let server = server.clone();
            let _request: server_status::ServerStatusRequest = parse_params(params)?;

            responder.respond(server_status::status(&server))
        }
        "_rustcode/warm" => {
            let _request: WarmRequest = parse_params(params)?;

            connection.spawn(async move {
                crate::mcp::start_deferred_servers().await;
                Ok(())
            })?;
            responder.respond(serde_json::json!({}))
        }
        // `gh` can take seconds, so neither holds the dispatch loop.
        "_rustcode/pulls" => {
            let request: PullsRequest = parse_params(params)?;

            connection.spawn(async move {
                let listed = crate::pull_requests::list(
                    &request.cwd,
                    request.state.as_deref().unwrap_or("open"),
                    request.search.as_deref(),
                    request.limit.unwrap_or(30),
                )
                .await;
                responder.respond(match listed {
                    Ok(list) => serde_json::json!({ "list": list }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/view" => {
            let request: PullViewRequest = parse_params(params)?;

            connection.spawn(async move {
                let viewed = crate::pull_requests::view(&request.cwd, request.number).await;
                responder.respond(match viewed {
                    Ok(pull) => serde_json::json!({ "pull": pull }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/branch" => {
            let request: PullBranchRequest = parse_params(params)?;

            connection.spawn(async move {
                let prepared =
                    crate::pull_requests::prepare_branch(&request.cwd, request.number).await;
                responder.respond(match prepared {
                    Ok(branch) => serde_json::json!({ "branch": branch }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/state" => {
            let request: PullStateRequest = parse_params(params)?;

            connection.spawn(async move {
                    if let Some(number) = request.number {
                        let answer = match crate::pull_requests::by_number(&request.cwd, number).await {
                            Ok(Some(pull)) => serde_json::json!({ "branch": pull.branch.clone(), "pull": pull }),
                            Ok(None) => serde_json::json!({
                                "branch": null,
                                "pull": null,
                                "error": { "code": "not_allowed", "message": format!("pull request #{number} is not from a branch of this repository") }
                            }),
                            Err(error) => serde_json::json!({ "branch": null, "error": error }),
                        };
                        return responder.respond(answer);
                    }
                    let asked = request
                        .branch
                        .filter(|branch| !branch.trim().is_empty());
                    let branch = match asked {
                        Some(branch) => Some(branch),
                        // Resolving HEAD runs git, so it does not hold an async worker.
                        None => {
                            let cwd = request.cwd.clone();
                            match tokio::task::spawn_blocking(move || {
                                crate::pull_requests::current_branch(&cwd)
                            })
                            .await
                            {
                                Ok(branch) => branch,
                                Err(error) => {
                                    return responder.respond_with_error(
                                        agent_client_protocol::Error::internal_error()
                                            .data(error.to_string()),
                                    );
                                }
                            }
                        }
                    };
                    let Some(branch) = branch else {
                        return responder.respond(serde_json::json!({
                            "branch": null,
                            "pull": null,
                            "error": { "code": "not_a_repository", "message": "the folder is not on a git branch" }
                        }));
                    };
                    let answer = match crate::pull_requests::for_branch(&request.cwd, &branch).await {
                        Ok(pull) => serde_json::json!({ "branch": branch, "pull": pull }),
                        Err(error) => serde_json::json!({ "branch": branch, "error": error }),
                    };
                    responder.respond(answer)?;
                    Ok(())
                })
        }
        "_rustcode/pulls/merge/cancel" => {
            let request: PullMergeCancelRequest = parse_params(params)?;

            let cancelled = crate::pull_requests::cancel_merge(&request.merge_id);
            responder.respond(serde_json::json!({ "cancelled": cancelled }))
        }
        "_rustcode/pulls/merge" => {
            let request: PullMergeRequest = parse_params(params)?;

            // Registered here, in the order requests arrive, so a cancel that
            // follows this request always finds the merge it is for.
            let cancel = request
                .merge_id
                .as_deref()
                .map(crate::pull_requests::register_merge)
                .unwrap_or_default();
            connection.spawn(async move {
                let _tracked = crate::pull_requests::MergeGuard(request.merge_id.clone());
                let wait = std::time::Duration::from_millis(
                    request
                        .timeout_ms
                        .unwrap_or(crate::pull_requests::MAX_MERGE_WAIT_SECS * 1000)
                        .max(1000),
                );
                let answer = match crate::pull_requests::merge_branch_when_green(
                    &request.cwd,
                    request.number,
                    request.branch.clone(),
                    wait,
                    &cancel,
                )
                .await
                {
                    Ok(outcome) => {
                        let announced = match (outcome.merged, request.session_id) {
                            (true, Some(session_id)) => {
                                let number = outcome.number;
                                tokio::task::spawn_blocking(move || {
                                    crate::pull_watch::WatchStore::default_store()
                                        .and_then(|store| {
                                            store.claim_merge(&session_id, number).ok()
                                        })
                                        .unwrap_or(false)
                                })
                                .await
                                .unwrap_or(false)
                            }
                            _ => false,
                        };
                        serde_json::json!({ "merge": outcome, "announced": announced })
                    }
                    Err(error) => serde_json::json!({ "error": error }),
                };
                responder.respond(answer)?;
                Ok(())
            })
        }
        "_rustcode/worktrees/cleanup" => {
            let request: WorktreeCleanupRequest = parse_params(params)?;

            connection.spawn(async move {
                let cleaned = crate::worktrees::cleanup_resolving_rename(
                    request.path,
                    request.branch,
                    request.head_sha,
                    request.merged,
                )
                .await;
                match cleaned {
                    Ok(Ok(cleanup)) => {
                        responder.respond(serde_json::json!({ "cleanup": cleanup }))?
                    }
                    Ok(Err(error)) => responder.respond_with_error(error.into_acp())?,
                    Err(error) => responder.respond_with_error(
                        agent_client_protocol::Error::internal_error().data(error.to_string()),
                    )?,
                }
                Ok(())
            })
        }
        "_rustcode/pulls/diff" => {
            let request: PullDiffRequest = parse_params(params)?;

            connection.spawn(async move {
                let diff = crate::pull_requests::diff(&request.cwd, request.number).await;
                responder.respond(match diff {
                    Ok(diff) => serde_json::to_value(diff).unwrap_or_default(),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/references" => {
            let request: PullReferencesRequest = parse_params(params)?;

            connection.spawn(async move {
                let read = crate::pull_requests::references(&request.cwd).await;
                responder.respond(match read {
                    Ok(references) => serde_json::json!({ "references": references }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/thread" => {
            let request: PullThreadRequest = parse_params(params)?;

            connection.spawn(async move {
                let done = crate::pull_requests::act_on_thread(
                    &request.cwd,
                    &request.repository,
                    request.number,
                    &request.action,
                )
                .await;
                responder.respond(match done {
                    Ok(done) => serde_json::json!({ "done": done }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/review" => {
            let request: PullReviewRequest = parse_params(params)?;

            connection.spawn(async move {
                let posted = match crate::pull_requests::review_payload(
                    &request.commit_id,
                    &request.event,
                    &request.body,
                    &request.comments,
                ) {
                    Ok(payload) => {
                        crate::pull_requests::post_review(
                            &request.cwd,
                            &request.repository,
                            request.number,
                            &payload,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                responder.respond(match posted {
                    Ok(review) => serde_json::json!({ "review": review }),
                    Err(error) => serde_json::json!({ "error": error }),
                })?;
                Ok(())
            })
        }
        "_rustcode/pulls/watch/start" => {
            let request: PullWatchStartRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::PullWatches);
            start_pull_watching(&server.clients);
            let Some(store) = crate::pull_watch::WatchStore::default_store() else {
                return responder.respond(serde_json::json!({ "error": { "code": "failed", "message": "No config directory" } }));
            };
            let started = if request.link {
                match crate::pull_requests::watch_view_on(
                    &request.cwd,
                    &request.repo,
                    request.number,
                )
                .await
                {
                    Ok(value) => store.link(
                        &request.session_id,
                        &request.cwd,
                        &request.repo,
                        request.number,
                        &value,
                        chrono::Utc::now().timestamp(),
                    ),
                    Err(error) => Err(error.message),
                }
            } else {
                store.start(
                    &request.session_id,
                    &request.cwd,
                    request.number,
                    request.interests.unwrap_or_default(),
                    chrono::Utc::now().timestamp(),
                )
            };
            responder.respond(match started {
                Ok(watch) => serde_json::json!({ "watch": watch }),
                Err(message) => {
                    serde_json::json!({ "error": { "code": "failed", "message": message } })
                }
            })
        }
        "_rustcode/pulls/watch/stop" => {
            let request: PullWatchStopRequest = parse_params(params)?;

            let stopped = crate::pull_watch::WatchStore::default_store()
                .and_then(|store| {
                    store
                        .stop(&request.session_id, request.number, request.remove)
                        .ok()
                })
                .flatten();
            responder.respond(serde_json::json!({ "watch": stopped }))
        }
        "_rustcode/pulls/watches" => {
            let request: PullWatchesRequest = parse_params(params)?;

            // The app asks on launch: watches from before a restart
            // carry on from here.
            server
                .clients
                .subscribe(client_id, clients::Topic::PullWatches);
            start_pull_watching(&server.clients);
            if let (Some(number), Some(session_id)) = (request.take, request.session_id.as_deref())
            {
                let taken = crate::pull_watch::WatchStore::default_store()
                    .map(|store| store.take(session_id, number))
                    .transpose();
                return responder.respond(match taken {
                    Ok(Some(Some((watch, events)))) => {
                        serde_json::json!({ "watch": watch, "events": events })
                    }
                    Ok(_) => serde_json::json!({ "events": [] }),
                    Err(error) => serde_json::json!({ "error": error.to_string() }),
                });
            }
            let watches = list_pull_watches(request.session_id.as_deref());
            responder.respond(serde_json::json!({ "watches": watches }))
        }
        "_rustcode/workflows/list" => {
            let request: workflows::WorkflowListRequest = parse_params(params)?;

            server
                .clients
                .subscribe(client_id, clients::Topic::Workflows);
            let clients = server.clients.clone();
            connection
                .spawn(async move { responder.respond(workflows::list(request, &clients).await) })
        }
        "_rustcode/workflows/stop" => {
            let request: workflows::WorkflowStopRequest = parse_params(params)?;

            // Stopping waits for the workers to be interrupted.
            connection.spawn(async move { responder.respond(workflows::stop(request).await) })
        }
        "_rustcode/workflows/resume" => {
            let request: workflows::WorkflowResumeRequest = parse_params(params)?;
            let asker = server.clients.info(client_id).orchestration;
            let host = (asker >= 2)
                .then(|| {
                    super::preview::orchestration_client_for(
                        true,
                        &server.clients.session(&request.session_id),
                    )
                })
                .flatten()
                .map(|client| (client, asker));

            responder.respond(workflows::resume(request, host))
        }
        "_rustcode/workflows/wake" => {
            let request: workflows::WorkflowWakeRequest = parse_params(params)?;

            responder.respond(workflows::wake(&request))
        }
        // Each runs gh or the account store, so none holds the dispatch loop.
        "_rustcode/connections" => {
            let _request: connections::ConnectionsRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::snapshot_value().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/login" => {
            let _request: connections::GitHubLoginRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_login().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/wait" => {
            let _request: connections::GitHubWaitRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_wait().await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/logout" => {
            let request: connections::GitHubLogoutRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::github_logout(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/account/refresh" => {
            let request: connections::AccountRefreshRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::account_refresh(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/account/logout" => {
            let request: connections::AccountLogoutRequest = parse_params(params)?;

            connection.spawn(async move {
                match connections::account_logout(request).await {
                    Ok(value) => responder.respond(value)?,
                    Err(error) => responder.respond_with_error(error)?,
                }
                Ok(())
            })
        }
        "_rustcode/connections/github/cancel" => {
            let _request: connections::GitHubCancelRequest = parse_params(params)?;

            crate::connections::cancel_github_login();
            responder.respond(serde_json::Value::Null)
        }
        _ => {
            *unhandled = Some(agent_client_protocol::Dispatch::Request(
                agent_client_protocol::UntypedMessage { method, params },
                responder,
            ));
            Ok(())
        }
    }
}
