//! NeoLingua Jam WebSocket protocol (ported from poc/server/jam.ts).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

pub const LAUNCH_COUNTDOWN_SECONDS: u64 = 5;
pub const SECONDS_PER_GAP: u64 = 15;
pub const QUIZ_RESUME_COUNTDOWN_MS: u64 = 3000;
pub const RECONNECT_GRACE_MS: u64 = 60_000;

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

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

struct Peer {
    id: String,
    client_id: String,
    tx: mpsc::UnboundedSender<String>,
    role: JamRole,
    name: String,
    disconnected_at: Option<i64>,
    disconnect_abort: Option<AbortHandle>,
    conn_id: u64,
}

struct RoundAnswer {
    answers: HashMap<String, String>,
    #[allow(dead_code)]
    answered_at: i64,
    #[allow(dead_code)]
    response_ms: i64,
    #[allow(dead_code)]
    correct_count: i64,
    timed_out: bool,
}

struct PlayerScore {
    #[allow(dead_code)]
    client_id: String,
    peer_id: String,
    name: String,
    points: i64,
    correct_answers: i64,
    wrong_answers: i64,
    total_response_ms: i64,
    rounds_played: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuizStatus {
    Open,
    Countdown,
    Done,
}

struct QuizRound {
    id: String,
    segments: Vec<QuizSegment>,
    gaps: Vec<QuizGapInput>,
    options: Vec<String>,
    opened_at: i64,
    deadline_at: i64,
    resume_at: Option<i64>,
    answers: HashMap<String, RoundAnswer>,
    status: QuizStatus,
    deadline_abort: Option<AbortHandle>,
    resume_abort: Option<AbortHandle>,
}

struct Session {
    id: String,
    state: JamState,
    peers: Vec<Peer>,
    peer_seq: u64,
    next_conn_id: u64,
    countdown_abort: Option<AbortHandle>,
    quiz: Option<QuizRound>,
    scores: HashMap<String, PlayerScore>,
}

pub struct JamRegistry {
    sessions: Mutex<HashMap<String, Arc<Mutex<Session>>>>,
}

type SessionHandle = Arc<Mutex<Session>>;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn create_code(length: usize) -> String {
    let mut code = String::with_capacity(length);
    for i in 0..length {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(now_ms() as u64);
        hasher.write_usize(i);
        hasher.write_u64(code.len() as u64);
        let idx = (hasher.finish() as usize) % CODE_ALPHABET.len();
        code.push(CODE_ALPHABET[idx] as char);
    }
    code
}

fn normalize_session_quiz_interval(raw: f64) -> i64 {
    if !raw.is_finite() {
        return 60;
    }
    crate::db::normalize_quiz_interval(raw.round().max(0.0) as u32) as i64
}

fn empty_state(quiz_mode: bool, quiz_interval_seconds: f64) -> JamState {
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

    pub fn create_session(&self, quiz_mode: bool, quiz_interval_seconds: f64) -> SessionPublic {
        let mut map = self.sessions.lock().expect("jam lock");
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
        let state = session.lock().expect("session lock").state.clone();
        map.insert(id.clone(), session);
        SessionPublic {
            session_id: id,
            state,
        }
    }

    pub fn get_session(&self, id: &str) -> Option<SessionView> {
        let key = id.to_uppercase();
        let map = self.sessions.lock().expect("jam lock");
        let handle = map.get(&key)?;
        let session = handle.lock().expect("session lock");
        Some(SessionView {
            session_id: session.id.clone(),
            state: session.state.clone(),
            peers: session.peers.len(),
        })
    }

    fn get_handle(&self, id: &str) -> Option<SessionHandle> {
        let key = id.to_uppercase();
        let map = self.sessions.lock().expect("jam lock");
        map.get(&key).cloned()
    }

    fn remove_session(&self, id: &str) {
        let mut map = self.sessions.lock().expect("jam lock");
        map.remove(id);
    }
}

impl Default for JamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn send_text(peer: &Peer, payload: &impl Serialize) {
    if let Ok(text) = serde_json::to_string(payload) {
        let _ = peer.tx.send(text);
    }
}

fn is_peer_active(peer: &Peer) -> bool {
    peer.disconnected_at.is_none()
}

fn broadcast(session: &Session, payload: &impl Serialize, except_conn: Option<u64>) {
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

fn broadcast_all(session: &Session, payload: &impl Serialize) {
    broadcast(session, payload, None);
}

fn companions(session: &Session) -> Vec<&Peer> {
    session
        .peers
        .iter()
        .filter(|p| p.role == JamRole::Companion && is_peer_active(p))
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeersMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    companions: usize,
    displays: usize,
    total: usize,
    admin_id: Option<String>,
    admin_name: Option<String>,
}

fn peer_snapshot(session: &Session) -> PeersMessage {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    #[serde(flatten)]
    state: JamState,
}

fn state_message(session: &Session) -> StateMessage {
    StateMessage {
        msg_type: "state",
        state: session.state.clone(),
    }
}

fn clear_quiz_timers(round: &mut QuizRound) {
    if let Some(h) = round.deadline_abort.take() {
        h.abort();
    }
    if let Some(h) = round.resume_abort.take() {
        h.abort();
    }
}

fn normalize_answer(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '\'' | '\u{2019}' | '-'))
        .collect()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GapResult {
    gap_id: String,
    correct: bool,
    yours: String,
    answer: String,
    level: Option<String>,
}

fn grade_answers(round: &QuizRound, answers: &HashMap<String, String>) -> Vec<GapResult> {
    round
        .gaps
        .iter()
        .map(|gap| {
            let yours = answers.get(&gap.id).cloned().unwrap_or_default();
            let correct = normalize_answer(&yours) == normalize_answer(&gap.answer);
            GapResult {
                gap_id: gap.id.clone(),
                correct,
                yours,
                answer: gap.answer.clone(),
                level: gap.level.clone(),
            }
        })
        .collect()
}

fn score_key(peer: &Peer) -> &str {
    &peer.client_id
}

fn answer_key(peer: &Peer) -> &str {
    &peer.client_id
}

fn ensure_score(session: &mut Session, peer: &Peer) {
    let key = score_key(peer).to_string();
    let score = session
        .scores
        .entry(key.clone())
        .or_insert_with(|| PlayerScore {
            client_id: peer.client_id.clone(),
            peer_id: peer.id.clone(),
            name: peer.name.clone(),
            points: 0,
            correct_answers: 0,
            wrong_answers: 0,
            total_response_ms: 0,
            rounds_played: 0,
        });
    score.peer_id = peer.id.clone();
    score.name = peer.name.clone();
}

fn credit_round_score(
    session: &mut Session,
    peer: &Peer,
    correct_count: i64,
    wrong_count: i64,
    response_ms: i64,
    timed_out: bool,
) {
    ensure_score(session, peer);
    let score = session.scores.get_mut(score_key(peer)).expect("score");
    score.correct_answers += correct_count.max(0);
    score.wrong_answers += wrong_count.max(0);
    score.rounds_played += 1;
    if timed_out {
        return;
    }
    score.points += correct_count;
    score.total_response_ms += response_ms.max(0);
}

fn record_round_answer(
    session: &mut Session,
    peer: &Peer,
    answers: HashMap<String, String>,
    timed_out: bool,
) -> Vec<GapResult> {
    let (results, correct_count, wrong_count, response_ms) = {
        let Some(round) = session.quiz.as_mut() else {
            return Vec::new();
        };
        let answered_at = now_ms();
        let response_ms = if timed_out {
            (round.deadline_at - round.opened_at).max(0)
        } else {
            (answered_at - round.opened_at).max(0)
        };
        let results = grade_answers(round, &answers);
        let correct_count = results.iter().filter(|r| r.correct).count() as i64;
        let wrong_count = results.len() as i64 - correct_count;
        let key = answer_key(peer).to_string();
        round.answers.insert(
            key,
            RoundAnswer {
                answers,
                answered_at,
                response_ms,
                correct_count,
                timed_out,
            },
        );
        (results, correct_count, wrong_count, response_ms)
    };
    credit_round_score(
        session,
        peer,
        correct_count,
        wrong_count,
        response_ms,
        timed_out,
    );
    results
}

fn build_leaderboard(session: &mut Session) -> Vec<LeaderboardEntry> {
    let companion_ids: Vec<(String, String, String)> = companions(session)
        .into_iter()
        .map(|p| (p.id.clone(), p.client_id.clone(), p.name.clone()))
        .collect();
    for (id, client_id, name) in &companion_ids {
        let fake = Peer {
            id: id.clone(),
            client_id: client_id.clone(),
            tx: {
                // Dummy channel never used; ensure_score only needs identity fields.
                let (tx, _) = mpsc::unbounded_channel();
                tx
            },
            role: JamRole::Companion,
            name: name.clone(),
            disconnected_at: None,
            disconnect_abort: None,
            conn_id: 0,
        };
        ensure_score(session, &fake);
    }

    let mut sorted: Vec<&PlayerScore> = session.scores.values().collect();
    sorted.sort_by(|a, b| {
        b.points
            .cmp(&a.points)
            .then_with(|| a.total_response_ms.cmp(&b.total_response_ms))
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
        .into_iter()
        .enumerate()
        .map(|(index, entry)| LeaderboardEntry {
            rank: index + 1,
            peer_id: entry.peer_id.clone(),
            name: entry.name.clone(),
            points: entry.points,
            correct_answers: entry.correct_answers,
            wrong_answers: entry.wrong_answers,
            total_response_ms: entry.total_response_ms,
            rounds_played: entry.rounds_played,
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JamEndedMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    reason: &'static str,
    by: String,
    leaderboard: Vec<LeaderboardEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandOut {
    #[serde(rename = "type")]
    msg_type: &'static str,
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<f64>,
    t: f64,
    playing: bool,
    by: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ToastMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    message: String,
}

fn end_jam(session: &mut Session, reason: &'static str, by: String) {
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuizStartMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    round_id: String,
    segments: Vec<QuizSegment>,
    gap_ids: Vec<String>,
    options: Vec<String>,
    deadline_at: i64,
    seconds_per_gap: u64,
    gap_count: usize,
}

fn quiz_public_payload(round: &QuizRound) -> QuizStartMessage {
    QuizStartMessage {
        msg_type: "quizStart",
        round_id: round.id.clone(),
        segments: round.segments.clone(),
        gap_ids: round.gaps.iter().map(|g| g.id.clone()).collect(),
        options: round.options.clone(),
        deadline_at: round.deadline_at,
        seconds_per_gap: SECONDS_PER_GAP,
        gap_count: round.gaps.len(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QuizProgressMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
    round_id: String,
    answered: usize,
    total: usize,
    deadline_at: i64,
}

fn quiz_progress_payload(session: &Session, round: &QuizRound) -> QuizProgressMessage {
    let needed = companions(session);
    let total = needed.len().max(1);
    let answered = needed
        .iter()
        .filter(|peer| round.answers.contains_key(answer_key(peer)))
        .count();
    QuizProgressMessage {
        msg_type: "quizProgress",
        round_id: round.id.clone(),
        answered,
        total,
        deadline_at: round.deadline_at,
    }
}

fn finish_quiz_and_play(session: &mut Session) {
    let round_id = if let Some(mut round) = session.quiz.take() {
        clear_quiz_timers(&mut round);
        round.status = QuizStatus::Done;
        Some(round.id)
    } else {
        None
    };

    session.state.phase = JamPhase::Playing;
    session.state.playing = true;
    session.state.updated_at = now_ms();
    session.state.last_by = Some("Quiz".into());

    broadcast_all(
        session,
        &CommandOut {
            msg_type: "command",
            action: "play".into(),
            delta: None,
            t: session.state.t,
            playing: true,
            by: "Quiz".into(),
        },
    );
    broadcast_all(session, &state_message(session));
    broadcast_all(
        session,
        &serde_json::json!({
            "type": "quizEnded",
            "roundId": round_id,
        }),
    );
}

fn begin_quiz_resume_countdown(registry: &Arc<JamRegistry>, handle: &SessionHandle) {
    let mut session = handle.lock().expect("session lock");
    let round_id = {
        let Some(round) = session.quiz.as_mut() else {
            return;
        };
        if round.status != QuizStatus::Open {
            return;
        }

        round.status = QuizStatus::Countdown;
        clear_quiz_timers(round);

        let resume_at = now_ms() + QUIZ_RESUME_COUNTDOWN_MS as i64;
        round.resume_at = Some(resume_at);
        let round_id = round.id.clone();
        (round_id, resume_at)
    };
    let (round_id, resume_at) = round_id;
    let progress = session
        .quiz
        .as_ref()
        .map(|r| quiz_progress_payload(&session, r));

    broadcast_all(
        &session,
        &serde_json::json!({
            "type": "quizComplete",
            "roundId": round_id,
            "resumeAt": resume_at,
            "countdownMs": QUIZ_RESUME_COUNTDOWN_MS,
        }),
    );
    if let Some(progress) = progress {
        broadcast_all(&session, &progress);
    }

    let registry = Arc::clone(registry);
    let handle_clone = Arc::clone(handle);
    let expected_id = round_id;
    let join = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(QUIZ_RESUME_COUNTDOWN_MS)).await;
        let mut session = handle_clone.lock().expect("session lock");
        if session.quiz.as_ref().is_some_and(|q| q.id == expected_id) {
            finish_quiz_and_play(&mut session);
        }
        let _ = registry;
    });
    if let Some(round) = session.quiz.as_mut() {
        round.resume_abort = Some(join.abort_handle());
    }
}

fn maybe_complete_quiz(registry: &Arc<JamRegistry>, handle: &SessionHandle) {
    {
        let session = handle.lock().expect("session lock");
        let Some(round) = session.quiz.as_ref() else {
            return;
        };
        if round.status != QuizStatus::Open {
            return;
        }
        let needed = companions(&session);
        if needed.is_empty() {
            drop(session);
            begin_quiz_resume_countdown(registry, handle);
            return;
        }
        let all_answered = needed
            .iter()
            .all(|peer| round.answers.contains_key(answer_key(peer)));
        if !all_answered {
            return;
        }
    }
    begin_quiz_resume_countdown(registry, handle);
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuizProposeMsg {
    segments: Vec<QuizSegment>,
    gaps: Vec<QuizGapInput>,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    t: Option<f64>,
}

fn start_quiz(registry: &Arc<JamRegistry>, handle: &SessionHandle, msg: QuizProposeMsg) {
    let mut session = handle.lock().expect("session lock");
    if !session.state.quiz_mode {
        return;
    }
    if session.state.phase != JamPhase::Playing {
        return;
    }
    if session
        .quiz
        .as_ref()
        .is_some_and(|q| q.status != QuizStatus::Done)
    {
        return;
    }
    if msg.gaps.is_empty() || msg.segments.is_empty() {
        return;
    }

    let gaps: Vec<QuizGapInput> = msg
        .gaps
        .into_iter()
        .filter(|g| !g.id.is_empty())
        .take(3)
        .map(|g| QuizGapInput {
            id: g.id,
            answer: g.answer,
            level: g.level.filter(|l| !l.is_empty()),
        })
        .collect();
    if gaps.is_empty() {
        return;
    }

    let options: Vec<String> = {
        let filtered: Vec<String> = msg
            .options
            .into_iter()
            .filter(|o| !o.trim().is_empty())
            .take(12)
            .collect();
        if filtered.is_empty() {
            gaps.iter().map(|g| g.answer.clone()).collect()
        } else {
            filtered
        }
    };

    if let Some(t) = msg.t.filter(|t| t.is_finite()) {
        session.state.t = t.max(0.0);
    }

    let gap_count = gaps.len();
    let deadline_at = now_ms() + (gap_count as i64) * (SECONDS_PER_GAP as i64) * 1000;
    let opened_at = now_ms();
    let round_id = create_code(6);
    let round = QuizRound {
        id: round_id.clone(),
        segments: msg.segments,
        gaps,
        options,
        opened_at,
        deadline_at,
        resume_at: None,
        answers: HashMap::new(),
        status: QuizStatus::Open,
        deadline_abort: None,
        resume_abort: None,
    };

    session.quiz = Some(round);
    session.state.phase = JamPhase::Quiz;
    session.state.playing = false;
    session.state.updated_at = now_ms();
    session.state.last_by = Some("Quiz".into());

    let public = quiz_public_payload(session.quiz.as_ref().unwrap());
    let progress = quiz_progress_payload(&session, session.quiz.as_ref().unwrap());
    let toast_msg = if gap_count > 1 {
        format!("Quiz · {gap_count} mots")
    } else {
        format!("Quiz · {gap_count} mot")
    };

    broadcast_all(
        &session,
        &CommandOut {
            msg_type: "command",
            action: "pause".into(),
            delta: None,
            t: session.state.t,
            playing: false,
            by: "Quiz".into(),
        },
    );
    broadcast_all(&session, &state_message(&session));
    broadcast_all(&session, &public);
    broadcast_all(&session, &progress);
    broadcast_all(
        &session,
        &ToastMessage {
            msg_type: "toast",
            message: toast_msg,
        },
    );

    let registry = Arc::clone(registry);
    let handle_clone = Arc::clone(handle);
    let expected_id = round_id;
    let wait_ms = gap_count as u64 * SECONDS_PER_GAP * 1000;
    let join = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        {
            let mut session = handle_clone.lock().expect("session lock");
            let Some(round) = session.quiz.as_ref() else {
                return;
            };
            if round.id != expected_id || round.status != QuizStatus::Open {
                return;
            }
            let companion_snapshot: Vec<(
                String,
                String,
                String,
                mpsc::UnboundedSender<String>,
                u64,
            )> = companions(&session)
                .into_iter()
                .filter(|p| !round.answers.contains_key(answer_key(p)))
                .map(|p| {
                    (
                        p.id.clone(),
                        p.client_id.clone(),
                        p.name.clone(),
                        p.tx.clone(),
                        p.conn_id,
                    )
                })
                .collect();

            for (id, client_id, name, tx, conn_id) in companion_snapshot {
                let peer = Peer {
                    id,
                    client_id,
                    tx: tx.clone(),
                    role: JamRole::Companion,
                    name,
                    disconnected_at: None,
                    disconnect_abort: None,
                    conn_id,
                };
                let results = record_round_answer(&mut session, &peer, HashMap::new(), true);
                let round_id = session
                    .quiz
                    .as_ref()
                    .map(|q| q.id.clone())
                    .unwrap_or_default();
                let _ = tx.send(
                    serde_json::to_string(&serde_json::json!({
                        "type": "quizFeedback",
                        "roundId": round_id,
                        "timedOut": true,
                        "results": results,
                        "waiting": false,
                    }))
                    .unwrap_or_default(),
                );
            }
        }
        begin_quiz_resume_countdown(&registry, &handle_clone);
    });
    if let Some(round) = session.quiz.as_mut() {
        round.deadline_abort = Some(join.abort_handle());
    }
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

fn ensure_admin(session: &mut Session) {
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
    // Build a temporary peer view for assign_admin.
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ClientMessage {
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

fn normalize_client_id(raw: Option<&str>, fallback: &str) -> String {
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

fn sync_quiz_state_to_peer(session: &Session, peer: &Peer) {
    let Some(round) = session.quiz.as_ref() else {
        return;
    };
    if round.status != QuizStatus::Open && round.status != QuizStatus::Countdown {
        return;
    }

    send_text(peer, &quiz_public_payload(round));
    send_text(peer, &quiz_progress_payload(session, round));

    if let Some(prior) = round.answers.get(answer_key(peer)) {
        let needed = companions(session);
        let waiting = round.status == QuizStatus::Open
            && !needed
                .iter()
                .all(|p| round.answers.contains_key(answer_key(p)));
        send_text(
            peer,
            &serde_json::json!({
                "type": "quizFeedback",
                "roundId": round.id,
                "timedOut": prior.timed_out,
                "results": grade_answers(round, &prior.answers),
                "waiting": waiting,
            }),
        );
    }

    if round.status == QuizStatus::Countdown {
        if let Some(resume_at) = round.resume_at {
            send_text(
                peer,
                &serde_json::json!({
                    "type": "quizComplete",
                    "roundId": round.id,
                    "resumeAt": resume_at,
                    "countdownMs": (resume_at - now_ms()).max(0),
                }),
            );
        }
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

fn attach_peer(session: &mut Session, peer_idx: usize, resumed: bool) {
    let peer_role = session.peers[peer_idx].role;
    let peer_id = session.peers[peer_idx].id.clone();
    let peer_name = session.peers[peer_idx].name.clone();

    if peer_role == JamRole::Companion {
        // ensure_score needs peer borrow
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

fn replace_peer_socket(
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

fn soft_detach_peer(registry: &Arc<JamRegistry>, handle: &SessionHandle, peer_id: &str) {
    let mut session = handle.lock().expect("session lock");
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
        purge_peer(&registry_c, &handle_c, &peer_id_c);
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
        maybe_complete_quiz(registry, handle);
    }
}

fn purge_peer(registry: &Arc<JamRegistry>, handle: &SessionHandle, peer_id: &str) {
    let mut session = handle.lock().expect("session lock");
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
        registry.remove_session(&id);
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
        maybe_complete_quiz(registry, handle);
    }
}

fn start_launch_countdown(registry: &Arc<JamRegistry>, handle: &SessionHandle, by: String) {
    let mut session = handle.lock().expect("session lock");
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
        let mut session = handle_c.lock().expect("session lock");
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

#[derive(Debug, Deserialize)]
pub struct JamWsQuery {
    pub session: String,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<JamWsQuery>,
    State(registry): State<Arc<JamRegistry>>,
) -> impl IntoResponse {
    let session_id = query.session.to_uppercase();
    ws.on_upgrade(move |socket| handle_socket(socket, registry, session_id))
}

async fn handle_socket(socket: WebSocket, registry: Arc<JamRegistry>, session_id: String) {
    let Some(handle) = registry.get_handle(&session_id) else {
        let mut socket = socket;
        let _ = socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "error",
                    "code": "session_gone",
                    "message": "Déconnecté trop longtemps · la session n’est plus disponible",
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

                let mut session = handle.lock().expect("session lock");
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

                // Verify this connection is still the active one for the peer.
                {
                    let session = handle.lock().expect("session lock");
                    let Some(peer) = session.peers.iter().find(|p| p.id == *pid) else {
                        continue;
                    };
                    if peer.conn_id != conn_id {
                        continue;
                    }
                }

                handle_peer_message(&registry, &handle, pid, conn_id, other, &tx);
            }
        }
    }

    if let (Some(pid), Some(conn_id)) = (peer_id, peer_conn_id) {
        let should_detach = {
            let session = handle.lock().expect("session lock");
            session
                .peers
                .iter()
                .find(|p| p.id == pid)
                .is_some_and(|p| p.conn_id == conn_id)
        };
        if should_detach {
            soft_detach_peer(&registry, &handle, &pid);
        }
    }

    drop(tx);
    let _ = writer.await;
}

fn handle_peer_message(
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
            let mut session = handle.lock().expect("session lock");
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
                session.state.quiz_interval_seconds = normalize_session_quiz_interval(interval);
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
                let session = handle.lock().expect("session lock");
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
                        "message": "Seul l’admin peut lancer",
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
            start_launch_countdown(registry, handle, by);
        }
        ClientMessage::EndJam { by } => {
            let (role, is_admin, peer_name) = {
                let session = handle.lock().expect("session lock");
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
                        "message": "Seul l’admin peut terminer le jam",
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
            let mut session = handle.lock().expect("session lock");
            end_jam(&mut session, "admin", by);
        }
        ClientMessage::VideoEnded => {
            let role = {
                let session = handle.lock().expect("session lock");
                session
                    .peers
                    .iter()
                    .find(|p| p.id == peer_id)
                    .map(|p| p.role)
            };
            if role != Some(JamRole::Display) {
                return;
            }
            let mut session = handle.lock().expect("session lock");
            end_jam(&mut session, "video", "Fin de l’épisode".into());
        }
        ClientMessage::QuizPropose {
            segments,
            gaps,
            options,
            t,
        } => {
            let role = {
                let session = handle.lock().expect("session lock");
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
            );
        }
        ClientMessage::QuizAnswer { round_id, answers } => {
            let mut session = handle.lock().expect("session lock");
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
            maybe_complete_quiz(registry, handle);
            let _ = conn_id;
        }
        ClientMessage::Clock {
            t,
            playing,
            duration,
        } => {
            let mut session = handle.lock().expect("session lock");
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
            let mut session = handle.lock().expect("session lock");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_round() -> QuizRound {
        QuizRound {
            id: "r1".into(),
            segments: vec![],
            gaps: vec![
                QuizGapInput {
                    id: "g1".into(),
                    answer: "Hello".into(),
                    level: Some("B1".into()),
                },
                QuizGapInput {
                    id: "g2".into(),
                    answer: "world".into(),
                    level: None,
                },
            ],
            options: vec![],
            opened_at: 0,
            deadline_at: 15_000,
            resume_at: None,
            answers: HashMap::new(),
            status: QuizStatus::Open,
            deadline_abort: None,
            resume_abort: None,
        }
    }

    #[test]
    fn create_session_normalizes_quiz_interval() {
        let registry = JamRegistry::new();
        let ok = registry.create_session(true, 120.0);
        assert_eq!(ok.state.quiz_interval_seconds, 120);
        assert!(ok.state.quiz_mode);
        assert_eq!(ok.state.phase, JamPhase::Lobby);
        assert_eq!(ok.session_id.len(), 4);

        let fallback = registry.create_session(false, 7.0);
        assert_eq!(fallback.state.quiz_interval_seconds, 60);
        assert!(!fallback.state.quiz_mode);

        let nan = registry.create_session(true, f64::NAN);
        assert_eq!(nan.state.quiz_interval_seconds, 60);
    }

    #[test]
    fn normalize_answer_strips_noise() {
        assert_eq!(normalize_answer("  Hello! "), "hello");
        assert_eq!(normalize_answer("it's"), "it's");
        assert_eq!(normalize_answer("well-known"), "well-known");
    }

    #[test]
    fn grade_answers_compares_normalized() {
        let round = sample_round();
        let mut answers = HashMap::new();
        answers.insert("g1".into(), " hello ".into());
        answers.insert("g2".into(), "WORLD!".into());
        let results = grade_answers(&round, &answers);
        assert_eq!(results.len(), 2);
        assert!(results[0].correct);
        assert!(results[1].correct);

        let mut wrong = HashMap::new();
        wrong.insert("g1".into(), "bye".into());
        let results = grade_answers(&round, &wrong);
        assert!(!results[0].correct);
        assert!(!results[1].correct);
        assert_eq!(results[1].yours, "");
    }

    #[test]
    fn credit_round_score_skips_points_on_timeout() {
        let mut session = Session {
            id: "ABCD".into(),
            state: empty_state(true, 60.0),
            peers: Vec::new(),
            peer_seq: 0,
            next_conn_id: 0,
            countdown_abort: None,
            quiz: None,
            scores: HashMap::new(),
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let peer = Peer {
            id: "p1".into(),
            client_id: "c1".into(),
            tx,
            role: JamRole::Companion,
            name: "Alice".into(),
            disconnected_at: None,
            disconnect_abort: None,
            conn_id: 1,
        };

        credit_round_score(&mut session, &peer, 2, 0, 1000, true);
        let score = session.scores.get("c1").expect("score");
        assert_eq!(score.points, 0);
        assert_eq!(score.correct_answers, 2);
        assert_eq!(score.rounds_played, 1);
        assert_eq!(score.total_response_ms, 0);

        credit_round_score(&mut session, &peer, 1, 1, 500, false);
        let score = session.scores.get("c1").expect("score");
        assert_eq!(score.points, 1);
        assert_eq!(score.correct_answers, 3);
        assert_eq!(score.wrong_answers, 1);
        assert_eq!(score.rounds_played, 2);
        assert_eq!(score.total_response_ms, 500);
    }
}
