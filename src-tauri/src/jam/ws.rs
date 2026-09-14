//! WebSocket handler for the Jam protocol.

use super::peers::{
    answer_key, attach_peer, broadcast, broadcast_all, companions, end_jam,
    normalize_client_id, replace_peer_socket, soft_detach_peer, start_launch_countdown,
    state_message,
};
use super::quiz::{
    clear_quiz_timers, maybe_complete_quiz, quiz_progress_payload,
    record_round_answer, start_quiz,
};
use super::registry::{now_ms, JamRegistry, SessionHandle};
use super::types::{
    ClientMessage, CommandOut, JamPhase, JamRole, JamWsQuery, Peer, QuizProposeMsg,
    QuizStatus, ToastMessage,
};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<JamWsQuery>,
    State(registry): State<Arc<JamRegistry>>,
) -> impl IntoResponse {
    let session_id = query.session.to_uppercase();
    ws.on_upgrade(move |socket| handle_socket(socket, registry, session_id))
}

async fn handle_socket(socket: WebSocket, registry: Arc<JamRegistry>, session_id: String) {
    let Some(handle) = registry.get_handle(&session_id).await else {
        let mut socket = socket;
        let _ = socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "error",
                    "code": "session_gone",
                    "message": "Déconnecté trop longtemps · la session n'est plus disponible",
                })
                .to_string()
                .into(),
            ))
            .await;
        let _ = socket.close().await;
        return;
    };

    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut peer_id: Option<String> = None;
    let mut peer_conn_id: Option<u64> = None;

    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };

        let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) else {
            continue;
        };

        match client_msg {
            ClientMessage::Hello {
                role,
                name,
                client_id,
            } => {
                if peer_id.is_some() {
                    continue;
                }
                let role = if role == "display" {
                    JamRole::Display
                } else {
                    JamRole::Companion
                };
                let name = name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.chars().take(24).collect::<String>())
                    .unwrap_or_else(|| {
                        if role == JamRole::Display {
                            "TV".into()
                        } else {
                            "Compagnon".into()
                        }
                    });

                let mut session = handle.lock().await;
                session.peer_seq += 1;
                let seq = session.peer_seq;
                let fallback = format!("anon-{seq}");
                let client_id = normalize_client_id(client_id.as_deref(), &fallback);
                session.next_conn_id += 1;
                let conn_id = session.next_conn_id;

                if let Some(idx) = session
                    .peers
                    .iter()
                    .position(|p| p.client_id == client_id && p.role == role)
                {
                    replace_peer_socket(&mut session, idx, tx.clone(), name, conn_id);
                    peer_id = Some(session.peers[idx].id.clone());
                    peer_conn_id = Some(conn_id);
                    continue;
                }

                let reclaiming =
                    role == JamRole::Companion && session.scores.contains_key(&client_id);
                let id = format!("p{seq}");
                session.peers.push(Peer {
                    id: id.clone(),
                    client_id,
                    tx: tx.clone(),
                    role,
                    name,
                    disconnected_at: None,
                    disconnect_abort: None,
                    conn_id,
                });
                let idx = session.peers.len() - 1;
                attach_peer(&mut session, idx, reclaiming);
                peer_id = Some(id);
                peer_conn_id = Some(conn_id);
            }
            other => {
                let Some(ref pid) = peer_id else {
                    continue;
                };
                let Some(conn_id) = peer_conn_id else {
                    continue;
                };

                {
                    let session = handle.lock().await;
                    let Some(peer) = session.peers.iter().find(|p| p.id == *pid) else {
                        continue;
                    };
                    if peer.conn_id != conn_id {
                        continue;
                    }
                }

                handle_peer_message(&registry, &handle, pid, conn_id, other, &tx).await;
            }
        }
    }

    if let (Some(pid), Some(conn_id)) = (peer_id, peer_conn_id) {
        let should_detach = {
            let session = handle.lock().await;
            session
                .peers
                .iter()
                .find(|p| p.id == pid)
                .is_some_and(|p| p.conn_id == conn_id)
        };
        if should_detach {
            soft_detach_peer(&registry, &handle, &pid).await;
        }
    }

    drop(tx);
    let _ = writer.await;
}

async fn handle_peer_message(
    registry: &Arc<JamRegistry>,
    handle: &SessionHandle,
    peer_id: &str,
    conn_id: u64,
    msg: ClientMessage,
    tx: &mpsc::UnboundedSender<String>,
) {
    match msg {
        ClientMessage::Hello { .. } => {}
        ClientMessage::Load {
            episode_path,
            episode_title,
            series_title,
            season_label,
            quiz_mode,
            quiz_interval_seconds,
            by,
        } => {
            let path = episode_path.trim();
            if path.is_empty() {
                return;
            }
            let mut session = handle.lock().await;
            let peer_name = session
                .peers
                .iter()
                .find(|p| p.id == peer_id)
                .map(|p| p.name.clone())
                .unwrap_or_default();

            if let Some(h) = session.countdown_abort.take() {
                h.abort();
            }
            if let Some(mut quiz) = session.quiz.take() {
                clear_quiz_timers(&mut quiz);
            }
            session.scores.clear();
            session.state.episode_path = Some(path.to_string());
            session.state.episode_title = episode_title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            session.state.series_title = series_title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            session.state.season_label = season_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if let Some(qm) = quiz_mode {
                session.state.quiz_mode = qm;
            }
            if let Some(interval) = quiz_interval_seconds.filter(|v| v.is_finite()) {
                session.state.quiz_interval_seconds =
                    crate::db::normalize_quiz_interval(interval.round().max(0.0) as u32) as i64;
            }
            session.state.t = 0.0;
            session.state.playing = false;
            session.state.duration = 0.0;
            session.state.phase = JamPhase::Lobby;
            session.state.updated_at = now_ms();
            session.state.last_by = Some(
                by.as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&peer_name)
                    .to_string(),
            );
            let last_by = session.state.last_by.clone().unwrap_or_default();
            broadcast_all(&session, &state_message(&session));
            broadcast_all(
                &session,
                &ToastMessage {
                    msg_type: "toast",
                    message: format!("Épisode prêt · {last_by}"),
                },
            );
        }
        ClientMessage::Launch { by } => {
            let (role, is_admin, peer_name) = {
                let session = handle.lock().await;
                let Some(peer) = session.peers.iter().find(|p| p.id == peer_id) else {
                    return;
                };
                (
                    peer.role,
                    session.state.admin_id.as_deref() == Some(peer_id),
                    peer.name.clone(),
                )
            };
            if role != JamRole::Companion {
                return;
            }
            if !is_admin {
                let _ = tx.send(
                    serde_json::json!({
                        "type": "error",
                        "message": "Seul l'admin peut lancer",
                    })
                    .to_string(),
                );
                return;
            }
            let by = by
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&peer_name)
                .to_string();
            start_launch_countdown(registry, handle, by).await;
        }
        ClientMessage::EndJam { by } => {
            let (role, is_admin, peer_name) = {
                let session = handle.lock().await;
                let Some(peer) = session.peers.iter().find(|p| p.id == peer_id) else {
                    return;
                };
                (
                    peer.role,
                    session.state.admin_id.as_deref() == Some(peer_id),
                    peer.name.clone(),
                )
            };
            if role != JamRole::Companion {
                return;
            }
            if !is_admin {
                let _ = tx.send(
                    serde_json::json!({
                        "type": "error",
                        "message": "Seul l'admin peut terminer le jam",
                    })
                    .to_string(),
                );
                return;
            }
            let by = by
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&peer_name)
                .to_string();
            let mut session = handle.lock().await;
            end_jam(&mut session, "admin", by);
        }
        ClientMessage::VideoEnded => {
            let role = {
                let session = handle.lock().await;
                session
                    .peers
                    .iter()
                    .find(|p| p.id == peer_id)
                    .map(|p| p.role)
            };
            if role != Some(JamRole::Display) {
                return;
            }
            let mut session = handle.lock().await;
            end_jam(&mut session, "video", "Fin de l'épisode".into());
        }
        ClientMessage::QuizPropose {
            segments,
            gaps,
            options,
            t,
        } => {
            let role = {
                let session = handle.lock().await;
                if session.state.phase == JamPhase::Ended {
                    return;
                }
                session
                    .peers
                    .iter()
                    .find(|p| p.id == peer_id)
                    .map(|p| p.role)
            };
            if role != Some(JamRole::Display) {
                return;
            }
            start_quiz(
                registry,
                handle,
                QuizProposeMsg {
                    segments,
                    gaps,
                    options,
                    t,
                },
            )
            .await;
        }
        ClientMessage::QuizAnswer { round_id, answers } => {
            let mut session = handle.lock().await;
            let Some(peer) = session.peers.iter().find(|p| p.id == peer_id) else {
                return;
            };
            if peer.role != JamRole::Companion {
                return;
            }
            let Some(round) = session.quiz.as_ref() else {
                return;
            };
            if round.status != QuizStatus::Open || round.id != round_id {
                return;
            }
            if round.answers.contains_key(answer_key(peer)) {
                return;
            }

            let peer_snapshot = Peer {
                id: peer.id.clone(),
                client_id: peer.client_id.clone(),
                tx: peer.tx.clone(),
                role: peer.role,
                name: peer.name.clone(),
                disconnected_at: peer.disconnected_at,
                disconnect_abort: None,
                conn_id: peer.conn_id,
            };
            let results = record_round_answer(&mut session, &peer_snapshot, answers, false);
            let needed = companions(&session);
            let waiting = !needed.iter().all(|p| {
                session
                    .quiz
                    .as_ref()
                    .is_some_and(|r| r.answers.contains_key(answer_key(p)))
            });
            let rid = session
                .quiz
                .as_ref()
                .map(|q| q.id.clone())
                .unwrap_or_default();
            let progress = session
                .quiz
                .as_ref()
                .map(|r| quiz_progress_payload(&session, r));

            let _ = tx.send(
                serde_json::to_string(&serde_json::json!({
                    "type": "quizFeedback",
                    "roundId": rid,
                    "timedOut": false,
                    "results": results,
                    "waiting": waiting,
                }))
                .unwrap_or_default(),
            );
            if let Some(progress) = progress {
                broadcast_all(&session, &progress);
            }
            drop(session);
            maybe_complete_quiz(registry, handle).await;
            let _ = conn_id;
        }
        ClientMessage::Clock {
            t,
            playing,
            duration,
        } => {
            let mut session = handle.lock().await;
            let Some(peer) = session.peers.iter().find(|p| p.id == peer_id) else {
                return;
            };
            if peer.role != JamRole::Display {
                return;
            }
            if session.state.phase != JamPhase::Playing {
                return;
            }
            if !t.is_finite() {
                return;
            }
            session.state.t = t.max(0.0);
            session.state.playing = playing;
            if let Some(d) = duration.filter(|d| d.is_finite()) {
                session.state.duration = d.max(0.0);
            }
            session.state.updated_at = now_ms();
            broadcast(&session, &state_message(&session), Some(conn_id));
        }
        ClientMessage::Command {
            action,
            delta,
            t,
            by,
        } => {
            let mut session = handle.lock().await;
            if session.state.phase == JamPhase::Quiz || session.state.phase == JamPhase::Ended {
                return;
            }
            if session.state.phase != JamPhase::Playing {
                return;
            }
            let peer_name = session
                .peers
                .iter()
                .find(|p| p.id == peer_id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let by = by
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&peer_name)
                .to_string();
            session.state.last_by = Some(by.clone());
            session.state.updated_at = now_ms();

            let label = match action.as_str() {
                "play" => {
                    session.state.playing = true;
                    Some(format!("Lecture · {by}"))
                }
                "pause" => {
                    session.state.playing = false;
                    Some(format!("Pause · {by}"))
                }
                "toggle" => {
                    session.state.playing = !session.state.playing;
                    Some(format!(
                        "{} · {by}",
                        if session.state.playing {
                            "Lecture"
                        } else {
                            "Pause"
                        }
                    ))
                }
                "seekBy" => {
                    let d = delta.filter(|d| d.is_finite()).unwrap_or(0.0);
                    session.state.t = (session.state.t + d).max(0.0);
                    Some(format!("{}{}s · {by}", if d >= 0.0 { "+" } else { "" }, d))
                }
                "seek" => {
                    if let Some(seek_t) = t.filter(|t| t.is_finite()) {
                        session.state.t = seek_t.max(0.0);
                        Some(format!("Seek · {by}"))
                    } else {
                        None
                    }
                }
                _ => return,
            };

            if action == "seek" && label.is_none() {
                return;
            }

            broadcast_all(
                &session,
                &CommandOut {
                    msg_type: "command",
                    action: action.clone(),
                    delta,
                    t: session.state.t,
                    playing: session.state.playing,
                    by: by.clone(),
                },
            );
            broadcast_all(&session, &state_message(&session));
            if let Some(message) = label {
                broadcast_all(
                    &session,
                    &ToastMessage {
                        msg_type: "toast",
                        message,
                    },
                );
            }
        }
    }
}
