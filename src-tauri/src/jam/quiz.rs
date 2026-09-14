//! Quiz logic: grading, scoring, timers, and round management.

use super::peers::{answer_key, broadcast_all, companions, score_key, send_text};
use super::registry::{create_code, now_ms, JamRegistry, SessionHandle};
use super::types::{
    CommandOut, GapResult, JamPhase, JamRole, LeaderboardEntry, Peer, PlayerScore,
    QuizGapInput, QuizProgressMessage, QuizProposeMsg, QuizRound, QuizStartMessage,
    QuizStatus, Session, ToastMessage, QUIZ_RESUME_COUNTDOWN_MS, SECONDS_PER_GAP,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub(super) fn clear_quiz_timers(round: &mut QuizRound) {
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

pub(super) fn grade_answers(round: &QuizRound, answers: &HashMap<String, String>) -> Vec<GapResult> {
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

pub(super) fn ensure_score(session: &mut Session, peer: &Peer) {
    let key = score_key(peer).to_string();
    let score = session
        .scores
        .entry(key.clone())
        .or_insert_with(|| PlayerScore {
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

pub(super) fn record_round_answer(
    session: &mut Session,
    peer: &Peer,
    answers: HashMap<String, String>,
    timed_out: bool,
) -> Vec<GapResult> {
    use super::types::RoundAnswer;
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

pub(super) fn build_leaderboard(session: &mut Session) -> Vec<LeaderboardEntry> {
    let companion_ids: Vec<(String, String, String)> = companions(session)
        .into_iter()
        .map(|p| (p.id.clone(), p.client_id.clone(), p.name.clone()))
        .collect();
    for (id, client_id, name) in &companion_ids {
        let fake = Peer {
            id: id.clone(),
            client_id: client_id.clone(),
            tx: {
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

pub(super) fn quiz_public_payload(round: &QuizRound) -> QuizStartMessage {
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

pub(super) fn quiz_progress_payload(session: &Session, round: &QuizRound) -> QuizProgressMessage {
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
    broadcast_all(session, &super::peers::state_message(session));
    broadcast_all(
        session,
        &serde_json::json!({
            "type": "quizEnded",
            "roundId": round_id,
        }),
    );
}

pub(super) async fn begin_quiz_resume_countdown(registry: &Arc<JamRegistry>, handle: &SessionHandle) {
    let mut session = handle.lock().await;
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
        let mut session = handle_clone.lock().await;
        if session.quiz.as_ref().is_some_and(|q| q.id == expected_id) {
            finish_quiz_and_play(&mut session);
        }
        let _ = registry;
    });
    if let Some(round) = session.quiz.as_mut() {
        round.resume_abort = Some(join.abort_handle());
    }
}

pub(super) async fn maybe_complete_quiz(registry: &Arc<JamRegistry>, handle: &SessionHandle) {
    {
        let session = handle.lock().await;
        let Some(round) = session.quiz.as_ref() else {
            return;
        };
        if round.status != QuizStatus::Open {
            return;
        }
        let needed = companions(&session);
        if needed.is_empty() {
            drop(session);
            begin_quiz_resume_countdown(registry, handle).await;
            return;
        }
        let all_answered = needed
            .iter()
            .all(|peer| round.answers.contains_key(answer_key(peer)));
        if !all_answered {
            return;
        }
    }
    begin_quiz_resume_countdown(registry, handle).await;
}

pub(super) async fn start_quiz(registry: &Arc<JamRegistry>, handle: &SessionHandle, msg: QuizProposeMsg) {
    let mut session = handle.lock().await;
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
    broadcast_all(&session, &super::peers::state_message(&session));
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
            let mut session = handle_clone.lock().await;
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
        begin_quiz_resume_countdown(&registry, &handle_clone).await;
    });
    if let Some(round) = session.quiz.as_mut() {
        round.deadline_abort = Some(join.abort_handle());
    }
}

pub(super) fn sync_quiz_state_to_peer(session: &Session, peer: &Peer) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jam::types::{QuizGapInput, QuizRound, QuizStatus};

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
        use crate::jam::registry::empty_state;
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
