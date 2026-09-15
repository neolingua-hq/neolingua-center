//! Peer management: attach, detach, broadcast, admin assignment.

use super::quiz::{
    build_leaderboard, clear_quiz_timers, ensure_score, maybe_complete_quiz,
    sync_quiz_state_to_peer,
};
use super::registry::{now_ms, JamRegistry, SessionHandle};
use super::types::{
    CommandOut, JamEndedMessage, JamPhase, JamRole, Peer, PeersMessage, QuizStatus, Session,
    StateMessage, ToastMessage, LAUNCH_COUNTDOWN_SECONDS, RECONNECT_GRACE_MS,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub(super) fn send_text(peer: &Peer, payload: &impl Serialize) {
    if let Ok(text) = serde_json::to_string(payload) {
        let _ = peer.tx.send(text);
    }
}

pub(super) fn is_peer_active(peer: &Peer) -> bool {
    peer.disconnected_at.is_none()
}

pub(super) fn broadcast(session: &Session, payload: &impl Serialize, except_conn: Option<u64>) {
    let Ok(text) = serde_json::to_string(payload) else {
        return;
    };
    for peer in &session.peers {
        if !is_peer_active(peer) {
            continue;
        }
        if except_conn.is_some_and(|c| peer.conn_id == c) {
            continue;
        }
        let _ = peer.tx.send(text.clone());
    }
}

pub(super) fn broadcast_all(session: &Session, payload: &impl Serialize) {
    broadcast(session, payload, None);
}

pub(super) fn companions(session: &Session) -> Vec<&Peer> {
    session
        .peers
        .iter()
        .filter(|p| p.role == JamRole::Companion && is_peer_active(p))
        .collect()
}

pub(super) fn peer_snapshot(session: &Session) -> PeersMessage {
    let active: Vec<&Peer> = session.peers.iter().filter(|p| is_peer_active(p)).collect();
    PeersMessage {
        msg_type: "peers",
        companions: active
            .iter()
            .filter(|p| p.role == JamRole::Companion)
            .count(),
        displays: active.iter().filter(|p| p.role == JamRole::Display).count(),
        total: active.len(),
        admin_id: session.state.admin_id.clone(),
        admin_name: session.state.admin_name.clone(),
    }
}

pub(super) fn state_message(session: &Session) -> StateMessage {
    StateMessage {
        msg_type: "state",
        state: session.state.clone(),
    }
}

pub(super) fn score_key(peer: &Peer) -> &str {
    &peer.client_id
}

pub(super) fn answer_key(peer: &Peer) -> &str {
    &peer.client_id
}

pub(super) fn end_jam(session: &mut Session, reason: &'static str, by: String) {
    if session.state.phase == JamPhase::Ended {
        return;
    }
    if session.state.phase != JamPhase::Playing && session.state.phase != JamPhase::Quiz {
        return;
    }

    if let Some(h) = session.countdown_abort.take() {
        h.abort();
    }
    if let Some(mut quiz) = session.quiz.take() {
        clear_quiz_timers(&mut quiz);
        quiz.status = QuizStatus::Done;
    }

    session.state.phase = JamPhase::Ended;
    session.state.playing = false;
    session.state.updated_at = now_ms();
    session.state.last_by = Some(by.clone());

    let leaderboard = build_leaderboard(session);
    broadcast_all(
        session,
        &CommandOut {
            msg_type: "command",
            action: "pause".into(),
            delta: None,
            t: session.state.t,
            playing: false,
            by: by.clone(),
        },
    );
    broadcast_all(session, &state_message(session));
    broadcast_all(
        session,
        &JamEndedMessage {
            msg_type: "jamEnded",
            reason,
            by,
            leaderboard,
        },
    );
}

fn assign_admin(session: &mut Session, peer: Option<&Peer>) {
    session.state.admin_id = peer.map(|p| p.id.clone());
    session.state.admin_name = peer.map(|p| p.name.clone());
    session.state.updated_at = now_ms();
    broadcast_all(
        session,
        &serde_json::json!({
            "type": "admin",
            "adminId": session.state.admin_id,
            "adminName": session.state.admin_name,
        }),
    );
    broadcast_all(session, &peer_snapshot(session));
    broadcast_all(session, &state_message(session));
}

pub(super) fn ensure_admin(session: &mut Session) {
    let list: Vec<(String, String)> = companions(session)
        .into_iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    if list.is_empty() {
        assign_admin(session, None);
        return;
    }
    if session
        .state
        .admin_id
        .as_ref()
        .is_some_and(|id| list.iter().any(|(pid, _)| pid == id))
    {
        return;
    }
    let (id, name) = &list[0];
    let (tx, _) = mpsc::unbounded_channel();
    let peer = Peer {
        id: id.clone(),
        client_id: String::new(),
        tx,
        role: JamRole::Companion,
        name: name.clone(),
        disconnected_at: None,
        disconnect_abort: None,
        conn_id: 0,
    };
    assign_admin(session, Some(&peer));
}

pub(super) fn normalize_client_id(raw: Option<&str>, fallback: &str) -> String {
    let Some(s) = raw else {
        return fallback.to_string();
    };
    let trimmed: String = s.trim().chars().take(80).collect();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

fn send_joined(session: &Session, peer: &Peer, resumed: bool) {
    send_text(
        peer,
        &serde_json::json!({
            "type": "joined",
            "sessionId": session.id,
            "role": peer.role,
            "name": peer.name,
            "peerId": peer.id,
            "isAdmin": peer.id == session.state.admin_id.clone().unwrap_or_default(),
            "resumed": resumed,
        }),
    );
}

pub(super) fn attach_peer(session: &mut Session, peer_idx: usize, resumed: bool) {
    let peer_role = session.peers[peer_idx].role;
    let peer_id = session.peers[peer_idx].id.clone();
    let peer_name = session.peers[peer_idx].name.clone();

    if peer_role == JamRole::Companion {
        let client_id = session.peers[peer_idx].client_id.clone();
        let (tx, _) = mpsc::unbounded_channel();
        let tmp = Peer {
            id: peer_id.clone(),
            client_id,
            tx,
            role: peer_role,
            name: peer_name.clone(),
            disconnected_at: None,
            disconnect_abort: None,
            conn_id: 0,
        };
        ensure_score(session, &tmp);
        if session.state.admin_id.is_none() {
            session.state.admin_id = Some(peer_id.clone());
            session.state.admin_name = Some(peer_name);
        }
    }

    send_joined(session, &session.peers[peer_idx], resumed);
    send_text(&session.peers[peer_idx], &state_message(session));
    broadcast_all(session, &peer_snapshot(session));
    if peer_role == JamRole::Companion
        && session.state.admin_id.as_deref() == Some(peer_id.as_str())
    {
        broadcast_all(
            session,
            &serde_json::json!({
                "type": "admin",
                "adminId": session.state.admin_id,
                "adminName": session.state.admin_name,
            }),
        );
    }
    sync_quiz_state_to_peer(session, &session.peers[peer_idx]);
    if session.state.phase == JamPhase::Ended {
        let leaderboard = build_leaderboard(session);
        send_text(
            &session.peers[peer_idx],
            &JamEndedMessage {
                msg_type: "jamEnded",
                reason: "admin",
                by: session
                    .state
                    .last_by
                    .clone()
                    .unwrap_or_else(|| "Jam".into()),
                leaderboard,
            },
        );
    }
}

pub(super) fn replace_peer_socket(
    session: &mut Session,
    peer_idx: usize,
    tx: mpsc::UnboundedSender<String>,
    name: String,
    conn_id: u64,
) {
    {
        let peer = &mut session.peers[peer_idx];
        if let Some(h) = peer.disconnect_abort.take() {
            h.abort();
        }
        peer.disconnected_at = None;
        peer.tx = tx;
        peer.name = name;
        peer.conn_id = conn_id;
    }
    if session.peers[peer_idx].role == JamRole::Companion {
        let id = session.peers[peer_idx].id.clone();
        let client_id = session.peers[peer_idx].client_id.clone();
        let name = session.peers[peer_idx].name.clone();
        let (dummy, _) = mpsc::unbounded_channel();
        ensure_score(
            session,
            &Peer {
                id,
                client_id,
                tx: dummy,
                role: JamRole::Companion,
                name,
                disconnected_at: None,
                disconnect_abort: None,
                conn_id: 0,
            },
        );
    }
    if session.state.admin_id.as_deref() == Some(session.peers[peer_idx].id.as_str()) {
        session.state.admin_name = Some(session.peers[peer_idx].name.clone());
    }

    send_joined(session, &session.peers[peer_idx], true);
    send_text(&session.peers[peer_idx], &state_message(session));
    broadcast_all(session, &peer_snapshot(session));
    let peer_id = session.peers[peer_idx].id.clone();
    let peer_role = session.peers[peer_idx].role;
    if peer_role == JamRole::Companion
        && session.state.admin_id.as_deref() == Some(peer_id.as_str())
    {
        broadcast_all(
            session,
            &serde_json::json!({
                "type": "admin",
                "adminId": session.state.admin_id,
                "adminName": session.state.admin_name,
            }),
        );
    }
    sync_quiz_state_to_peer(session, &session.peers[peer_idx]);
    if session.state.phase == JamPhase::Ended {
        let leaderboard = build_leaderboard(session);
        send_text(
            &session.peers[peer_idx],
            &JamEndedMessage {
                msg_type: "jamEnded",
                reason: "admin",
                by: session
                    .state
                    .last_by
                    .clone()
                    .unwrap_or_else(|| "Jam".into()),
                leaderboard,
            },
        );
    }
}

pub(super) async fn soft_detach_peer(
    registry: &Arc<JamRegistry>,
    handle: &SessionHandle,
    peer_id: &str,
) {
    let mut session = handle.lock().await;
    let Some(peer) = session.peers.iter_mut().find(|p| p.id == peer_id) else {
        return;
    };
    if peer.disconnected_at.is_some() {
        return;
    }
    peer.disconnected_at = Some(now_ms());
    if let Some(h) = peer.disconnect_abort.take() {
        h.abort();
    }

    let registry_c = Arc::clone(registry);
    let handle_c = Arc::clone(handle);
    let peer_id_c = peer_id.to_string();
    let join = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(RECONNECT_GRACE_MS)).await;
        purge_peer(&registry_c, &handle_c, &peer_id_c).await;
    });
    if let Some(peer) = session.peers.iter_mut().find(|p| p.id == peer_id) {
        peer.disconnect_abort = Some(join.abort_handle());
    }

    broadcast_all(&session, &peer_snapshot(&session));
    let quiz_open = session
        .quiz
        .as_ref()
        .is_some_and(|q| q.status == QuizStatus::Open);
    drop(session);
    if quiz_open {
        maybe_complete_quiz(registry, handle).await;
    }
}

pub(super) async fn purge_peer(registry: &Arc<JamRegistry>, handle: &SessionHandle, peer_id: &str) {
    let mut session = handle.lock().await;
    let Some(idx) = session.peers.iter().position(|p| p.id == peer_id) else {
        return;
    };
    if session.peers[idx].disconnected_at.is_none() {
        return;
    }

    let was_admin = session.state.admin_id.as_deref() == Some(peer_id);
    session.peers.remove(idx);

    if session.peers.is_empty() {
        if let Some(h) = session.countdown_abort.take() {
            h.abort();
        }
        if let Some(mut quiz) = session.quiz.take() {
            clear_quiz_timers(&mut quiz);
        }
        let id = session.id.clone();
        drop(session);
        registry.remove_session(&id).await;
        return;
    }

    if !session.peers.iter().any(is_peer_active) {
        return;
    }

    if was_admin {
        ensure_admin(&mut session);
    } else {
        broadcast_all(&session, &peer_snapshot(&session));
    }
    let quiz_open = session
        .quiz
        .as_ref()
        .is_some_and(|q| q.status == QuizStatus::Open);
    drop(session);
    if quiz_open {
        maybe_complete_quiz(registry, handle).await;
    }
}

pub(super) async fn start_launch_countdown(
    registry: &Arc<JamRegistry>,
    handle: &SessionHandle,
    by: String,
) {
    let mut session = handle.lock().await;
    if session.state.phase != JamPhase::Lobby {
        return;
    }
    if session.state.episode_path.is_none() {
        return;
    }

    if let Some(h) = session.countdown_abort.take() {
        h.abort();
    }

    session.state.phase = JamPhase::Countdown;
    session.state.playing = false;
    session.state.last_by = Some(by.clone());
    session.state.updated_at = now_ms();

    let ends_at = now_ms() + (LAUNCH_COUNTDOWN_SECONDS as i64) * 1000;
    broadcast_all(
        &session,
        &serde_json::json!({
            "type": "countdown",
            "seconds": LAUNCH_COUNTDOWN_SECONDS,
            "endsAt": ends_at,
            "by": by,
        }),
    );
    broadcast_all(&session, &state_message(&session));
    broadcast_all(
        &session,
        &ToastMessage {
            msg_type: "toast",
            message: format!("Lancement · {by}"),
        },
    );

    let handle_c = Arc::clone(handle);
    let by_c = by;
    let _registry = Arc::clone(registry);
    let join = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(LAUNCH_COUNTDOWN_SECONDS)).await;
        let mut session = handle_c.lock().await;
        session.countdown_abort = None;
        if session.state.phase != JamPhase::Countdown {
            return;
        }
        session.state.phase = JamPhase::Playing;
        session.state.playing = true;
        session.state.t = 0.0;
        session.state.updated_at = now_ms();
        session.state.last_by = Some(by_c.clone());

        broadcast_all(
            &session,
            &serde_json::json!({
                "type": "go",
                "by": by_c,
            }),
        );
        broadcast_all(
            &session,
            &CommandOut {
                msg_type: "command",
                action: "play".into(),
                delta: None,
                t: 0.0,
                playing: true,
                by: by_c,
            },
        );
        broadcast_all(&session, &state_message(&session));
    });
    session.countdown_abort = Some(join.abort_handle());
}
