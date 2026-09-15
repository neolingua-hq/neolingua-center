//! JamRegistry: session creation, lookup, and removal.

use super::types::{JamPhase, JamState, Session, SessionPublic, SessionView, CODE_ALPHABET};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

pub type SessionHandle = Arc<Mutex<Session>>;

pub struct JamRegistry {
    sessions: Mutex<HashMap<String, SessionHandle>>,
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn create_code(length: usize) -> String {
    let mut rng = rand::rng();
    use rand::Rng;
    (0..length)
        .map(|_| {
            let idx = rng.random_range(0..CODE_ALPHABET.len());
            CODE_ALPHABET[idx] as char
        })
        .collect()
}

fn normalize_session_quiz_interval(raw: f64) -> i64 {
    if !raw.is_finite() {
        return 60;
    }
    crate::db::normalize_quiz_interval(raw.round().max(0.0) as u32) as i64
}

pub(super) fn empty_state(quiz_mode: bool, quiz_interval_seconds: f64) -> JamState {
    let interval = normalize_session_quiz_interval(quiz_interval_seconds);
    JamState {
        episode_path: None,
        episode_title: None,
        series_title: None,
        season_label: None,
        t: 0.0,
        playing: false,
        duration: 0.0,
        updated_at: now_ms(),
        last_by: None,
        phase: JamPhase::Lobby,
        admin_id: None,
        admin_name: None,
        quiz_mode,
        quiz_interval_seconds: interval,
    }
}

impl JamRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn create_session(
        &self,
        quiz_mode: bool,
        quiz_interval_seconds: f64,
    ) -> SessionPublic {
        let mut map = self.sessions.lock().await;
        let mut id = create_code(4);
        while map.contains_key(&id) {
            id = create_code(4);
        }
        let session = Arc::new(Mutex::new(Session {
            id: id.clone(),
            state: empty_state(quiz_mode, quiz_interval_seconds),
            peers: Vec::new(),
            peer_seq: 0,
            next_conn_id: 0,
            countdown_abort: None,
            quiz: None,
            scores: HashMap::new(),
        }));
        let state = session.lock().await.state.clone();
        map.insert(id.clone(), session);
        SessionPublic {
            session_id: id,
            state,
        }
    }

    pub async fn get_session(&self, id: &str) -> Option<SessionView> {
        let key = id.to_uppercase();
        let map = self.sessions.lock().await;
        let handle = map.get(&key)?;
        let session = handle.lock().await;
        Some(SessionView {
            session_id: session.id.clone(),
            state: session.state.clone(),
            peers: session.peers.len(),
        })
    }

    pub(super) async fn get_handle(&self, id: &str) -> Option<SessionHandle> {
        let key = id.to_uppercase();
        let map = self.sessions.lock().await;
        map.get(&key).cloned()
    }

    pub(super) async fn remove_session(&self, id: &str) {
        let mut map = self.sessions.lock().await;
        map.remove(id);
    }
}

impl Default for JamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_session_normalizes_quiz_interval() {
        let registry = JamRegistry::new();
        let ok = registry.create_session(true, 120.0).await;
        assert_eq!(ok.state.quiz_interval_seconds, 120);
        assert!(ok.state.quiz_mode);
        assert_eq!(ok.state.phase, JamPhase::Lobby);
        assert_eq!(ok.session_id.len(), 4);

        let fallback = registry.create_session(false, 7.0).await;
        assert_eq!(fallback.state.quiz_interval_seconds, 60);
        assert!(!fallback.state.quiz_mode);

        let nan = registry.create_session(true, f64::NAN).await;
        assert_eq!(nan.state.quiz_interval_seconds, 60);
    }
}
