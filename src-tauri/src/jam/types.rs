//! Data types for the NeoLingua Jam WebSocket protocol.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

pub const LAUNCH_COUNTDOWN_SECONDS: u64 = 5;
pub const SECONDS_PER_GAP: u64 = 15;
pub const QUIZ_RESUME_COUNTDOWN_MS: u64 = 3000;
pub const RECONNECT_GRACE_MS: u64 = 60_000;

pub(super) const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JamRole {
    Display,
    Companion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JamPhase {
    Lobby,
    Countdown,
    Playing,
    Quiz,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JamState {
    pub episode_path: Option<String>,
    pub episode_title: Option<String>,
    pub series_title: Option<String>,
    pub season_label: Option<String>,
    pub t: f64,
    pub playing: bool,
    pub duration: f64,
    pub updated_at: i64,
    pub last_by: Option<String>,
    pub phase: JamPhase,
    pub admin_id: Option<String>,
    pub admin_name: Option<String>,
    pub quiz_mode: bool,
    pub quiz_interval_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum QuizSegment {
    #[serde(rename_all = "camelCase")]
    Text { value: String },
    #[serde(rename_all = "camelCase")]
    Gap { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuizGapInput {
    pub id: String,
    pub answer: String,
    #[serde(default)]
    pub level: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaderboardEntry {
    pub rank: usize,
    pub peer_id: String,
    pub name: String,
    pub points: i64,
    pub correct_answers: i64,
    pub wrong_answers: i64,
    pub total_response_ms: i64,
    pub rounds_played: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPublic {
    pub session_id: String,
    pub state: JamState,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub session_id: String,
    pub state: JamState,
    pub peers: usize,
}

pub(super) struct Peer {
    pub id: String,
    pub client_id: String,
    pub tx: mpsc::UnboundedSender<String>,
    pub role: JamRole,
    pub name: String,
    pub disconnected_at: Option<i64>,
    pub disconnect_abort: Option<AbortHandle>,
    pub conn_id: u64,
}

pub(super) struct RoundAnswer {
    pub answers: HashMap<String, String>,
    pub timed_out: bool,
}

pub(super) struct PlayerScore {
    pub peer_id: String,
    pub name: String,
    pub points: i64,
    pub correct_answers: i64,
    pub wrong_answers: i64,
    pub total_response_ms: i64,
    pub rounds_played: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QuizStatus {
    Open,
    Countdown,
    Done,
}

pub(super) struct QuizRound {
    pub id: String,
    pub segments: Vec<QuizSegment>,
    pub gaps: Vec<QuizGapInput>,
    pub options: Vec<String>,
    pub opened_at: i64,
    pub deadline_at: i64,
    pub resume_at: Option<i64>,
    pub answers: HashMap<String, RoundAnswer>,
    pub status: QuizStatus,
    pub deadline_abort: Option<AbortHandle>,
    pub resume_abort: Option<AbortHandle>,
}

pub(super) struct Session {
    pub id: String,
    pub state: JamState,
    pub peers: Vec<Peer>,
    pub peer_seq: u64,
    pub next_conn_id: u64,
    pub countdown_abort: Option<AbortHandle>,
    pub quiz: Option<QuizRound>,
    pub scores: HashMap<String, PlayerScore>,
}

// Message structs for serialization

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PeersMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub companions: usize,
    pub displays: usize,
    pub total: usize,
    pub admin_id: Option<String>,
    pub admin_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StateMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    #[serde(flatten)]
    pub state: JamState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JamEndedMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub reason: &'static str,
    pub by: String,
    pub leaderboard: Vec<LeaderboardEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommandOut {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    pub t: f64,
    pub playing: bool,
    pub by: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ToastMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuizStartMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub round_id: String,
    pub segments: Vec<QuizSegment>,
    pub gap_ids: Vec<String>,
    pub options: Vec<String>,
    pub deadline_at: i64,
    pub seconds_per_gap: u64,
    pub gap_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuizProgressMessage {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub round_id: String,
    pub answered: usize,
    pub total: usize,
    pub deadline_at: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GapResult {
    pub gap_id: String,
    pub correct: bool,
    pub yours: String,
    pub answer: String,
    pub level: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum ClientMessage {
    #[serde(rename_all = "camelCase")]
    Hello {
        role: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        client_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Load {
        episode_path: String,
        #[serde(default)]
        episode_title: Option<String>,
        #[serde(default)]
        series_title: Option<String>,
        #[serde(default)]
        season_label: Option<String>,
        #[serde(default)]
        quiz_mode: Option<bool>,
        #[serde(default)]
        quiz_interval_seconds: Option<f64>,
        #[serde(default)]
        by: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Clock {
        t: f64,
        playing: bool,
        #[serde(default)]
        duration: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    Command {
        action: String,
        #[serde(default)]
        delta: Option<f64>,
        #[serde(default)]
        t: Option<f64>,
        #[serde(default)]
        by: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Launch {
        #[serde(default)]
        by: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    EndJam {
        #[serde(default)]
        by: Option<String>,
    },
    VideoEnded,
    #[serde(rename_all = "camelCase")]
    QuizPropose {
        segments: Vec<QuizSegment>,
        gaps: Vec<QuizGapInput>,
        #[serde(default)]
        options: Vec<String>,
        #[serde(default)]
        t: Option<f64>,
    },
    #[serde(rename_all = "camelCase")]
    QuizAnswer {
        round_id: String,
        #[serde(default)]
        answers: HashMap<String, String>,
    },
}

#[derive(Debug, Deserialize)]
pub struct JamWsQuery {
    pub session: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuizProposeMsg {
    pub segments: Vec<QuizSegment>,
    pub gaps: Vec<QuizGapInput>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub t: Option<f64>,
}
