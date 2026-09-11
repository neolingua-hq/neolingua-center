use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// Generous TTL: browser background tabs throttle timers heavily.
const SESSION_TTL: Duration = Duration::from_secs(75);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WatchMode {
    Solo,
    Jam,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSession {
    pub client_id: String,
    pub mode: WatchMode,
    pub title: Option<String>,
    pub path: Option<String>,
    pub playing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSnapshot {
    pub sessions: Vec<PresenceSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceHeartbeat {
    pub client_id: String,
    pub mode: WatchMode,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default = "default_playing")]
    pub playing: bool,
}

fn default_playing() -> bool {
    true
}

struct LiveSession {
    mode: WatchMode,
    title: Option<String>,
    path: Option<String>,
    playing: bool,
    updated_at: Instant,
}

pub struct PresenceRegistry {
    inner: Mutex<HashMap<String, LiveSession>>,
}

impl PresenceRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn heartbeat(&self, body: PresenceHeartbeat) {
        let client_id = body.client_id.trim().to_string();
        if client_id.is_empty() {
            return;
        }
        let mut map = self.inner.lock().expect("presence lock");
        Self::prune_locked(&mut map);
        if !body.playing {
            map.remove(&client_id);
            return;
        }
        map.insert(
            client_id,
            LiveSession {
                mode: body.mode,
                title: body
                    .title
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
                path: body
                    .path
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty()),
                playing: true,
                updated_at: Instant::now(),
            },
        );
    }

    pub fn snapshot(&self) -> PresenceSnapshot {
        let mut map = self.inner.lock().expect("presence lock");
        Self::prune_locked(&mut map);
        let mut sessions: Vec<PresenceSession> = map
            .iter()
            .map(|(client_id, live)| PresenceSession {
                client_id: client_id.clone(),
                mode: live.mode.clone(),
                title: live.title.clone(),
                path: live.path.clone(),
                playing: live.playing,
            })
            .collect();
        sessions.sort_by(|a, b| a.client_id.cmp(&b.client_id));
        PresenceSnapshot { sessions }
    }

    fn prune_locked(map: &mut HashMap<String, LiveSession>) {
        let now = Instant::now();
        map.retain(|_, live| now.duration_since(live.updated_at) <= SESSION_TTL);
    }
}

impl Default for PresenceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beat(client_id: &str, mode: WatchMode, title: Option<&str>, playing: bool) -> PresenceHeartbeat {
        PresenceHeartbeat {
            client_id: client_id.into(),
            mode,
            title: title.map(str::to_string),
            path: None,
            playing,
        }
    }

    #[test]
    fn ignores_empty_client_and_stops_on_playing_false() {
        let reg = PresenceRegistry::new();
        reg.heartbeat(beat("  ", WatchMode::Solo, Some("x"), true));
        assert!(reg.snapshot().sessions.is_empty());

        reg.heartbeat(beat("tv", WatchMode::Jam, Some("  Foundation  "), true));
        let snap = reg.snapshot();
        assert_eq!(snap.sessions.len(), 1);
        assert_eq!(snap.sessions[0].title.as_deref(), Some("Foundation"));

        reg.heartbeat(beat("tv", WatchMode::Jam, None, false));
        assert!(reg.snapshot().sessions.is_empty());
    }

    #[test]
    fn snapshot_sorted_by_client_id() {
        let reg = PresenceRegistry::new();
        reg.heartbeat(beat("z", WatchMode::Solo, None, true));
        reg.heartbeat(beat("a", WatchMode::Jam, None, true));
        let ids: Vec<_> = reg
            .snapshot()
            .sessions
            .into_iter()
            .map(|s| s.client_id)
            .collect();
        assert_eq!(ids, vec!["a".to_string(), "z".to_string()]);
    }
}

