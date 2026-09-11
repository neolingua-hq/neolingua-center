/**
 * NeoLingua Center Jam client (vanilla ES module).
 * Companion UI for Jam; quiz rounds are proposed by the display host.
 */

import {
  annotatePlainCue,
  buildQuizSchedule,
  findScheduledQuiz,
  QUIZ_FIRST_AFTER_SECONDS,
  renderGappedCueHtml,
} from "./jam-gaps.js";
import { applyLeaderboard } from "./leaderboard.js";
import {
  SIZE_STEPS,
  clampSizeIndex,
  detectIntroRangeSeconds,
  escapeHtml,
  formatTime,
  parseVtt,
} from "./media-utils.js";
import { PRESENCE_INTERVAL_MS, viewerClientId } from "./presence-client.js";
import { renderQuizSentenceHtml } from "./jam-quiz-ui.js";

const SUB_EN_KEY = "neolingua.jam.sub.showEn";
const SUB_FR_KEY = "neolingua.jam.sub.showFr";
const SUB_EN_SIZE_KEY = "neolingua.jam.sub.sizeEn";
const SUB_FR_SIZE_KEY = "neolingua.jam.sub.sizeFr";
const NAME_KEY = "neolingua.jam.name";

const params = new URLSearchParams(window.location.search);
const joinCode = (params.get("s") ?? params.get("session") ?? "").toUpperCase();
const forcedRole = params.get("role");
const role =
  forcedRole === "companion" || joinCode
    ? "companion"
    : forcedRole === "display"
      ? "display"
      : "companion";

// Catalog browse + Jam host live on `/` (clean URLs). This page is companions only.
if (!joinCode && forcedRole !== "companion") {
  window.location.replace("/");
  throw new Error("redirecting to catalog");
}

document.documentElement.classList.add("is-jam-companion");

function lockMobileViewport() {
  if (role !== "companion") return;
  const meta = document.querySelector('meta[name="viewport"]');
  if (meta) {
    meta.setAttribute(
      "content",
      "width=device-width, initial-scale=1, minimum-scale=1, maximum-scale=1, user-scalable=no, viewport-fit=cover",
    );
  }
}
lockMobileViewport();

const CLIENT_ID_KEY = `neolingua.jam.clientId.${role}`;

function getJamClientId() {
  let id = localStorage.getItem(CLIENT_ID_KEY);
  if (!id) {
    id =
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `jam-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    localStorage.setItem(CLIENT_ID_KEY, id);
  }
  return id;
}

function peerStorageKey() {
  return `neolingua.jam.peerId.${sessionId || "none"}`;
}

function rememberPeerId(peerId) {
  if (!sessionId || !peerId) return;
  try {
    localStorage.setItem(peerStorageKey(), peerId);
  } catch {
    /* ignore */
  }
}

function recalledPeerId() {
  if (!sessionId) return "";
  try {
    return localStorage.getItem(peerStorageKey()) || "";
  } catch {
    return "";
  }
}

function clearRecalledPeerId() {
  if (!sessionId) return;
  try {
    localStorage.removeItem(peerStorageKey());
  } catch {
    /* ignore */
  }
}

function notifyLateDisconnect(kind) {
  if (kind === "gone") {
    showToast("Session terminée ou expirée");
    setStatus("Session plus disponible", "error");
  } else {
    showToast("Reconnecté avec un nouvel identifiant");
  }
}

const app = document.querySelector("#app");
if (!app) throw new Error("#app manquant");

let selectedEpisode = "";
let sessionId = joinCode;
let ws = null;
let myName = localStorage.getItem(NAME_KEY)?.trim() || "";
let myPeerId = "";
let reconnectEnabled = true;
let isAdmin = false;
let jamPhase = "lobby";
let adminName = null;
let companionCount = 0;

let cuesEn = [];
let cuesFr = [];
let hasEn = false;
let hasFr = false;
let showEn = localStorage.getItem(SUB_EN_KEY) !== "0";
let showFr = localStorage.getItem(SUB_FR_KEY) !== "0";
let sizeEnIndex = clampSizeIndex(Number(localStorage.getItem(SUB_EN_SIZE_KEY) ?? "3"));
let sizeFrIndex = clampSizeIndex(Number(localStorage.getItem(SUB_FR_SIZE_KEY) ?? "2"));
let loadedEpisode = "";

let clockT = 0;
let clockPlaying = false;
let clockDuration = 0;
let clockReceivedAt = performance.now();
let clockPollTimer = null;
let toastTimer = null;
let countdownTimer = null;
let applyingRemote = false;
let displayPlayer = null;
let wsGeneration = 0;
let audioCtx = null;
let videoReady = false;
/** @type {number | null} */
let introStartSeconds = null;
/** @type {number | null} */
let introEndSeconds = null;
let presenceTimer = null;
let presenceTitle = "";
let presencePath = "";

let quizSelections = {};
/** Option chip indexes already placed into a gap (allows duplicate words). */
let quizUsedOptionIndexes = new Set();
/** gapId → option index used to fill it */
let quizGapOptionIndex = {};
let quizAnswered = false;
let activeRoundId = null;
let quizTimerInterval = null;
let quizCountdownInterval = null;
let quizModeEnabled = false;
let quizIntervalSeconds = 60;
/** True after admin Launch while waiting for the TV tap (fullscreen gesture). */
let launchArmed = false;
/** @type {{ start: number, end: number, text: string, tokens: object[] }[]} */
let quizCues = [];
/** @type {{ cueStart: number, cueEnd: number, gapTokenIndexes: number[], proposal: object }[]} */
let quizSchedule = [];

function rebuildCompanionQuizSchedule(firstAfterSeconds = QUIZ_FIRST_AFTER_SECONDS) {
  if (!quizModeEnabled || quizCues.length === 0) {
    quizSchedule = [];
    return;
  }
  quizSchedule = buildQuizSchedule(quizCues, quizIntervalSeconds, firstAfterSeconds);
}

function headerHtml(extra = "") {
  return `
    <header class="header" id="display-header">
      <a class="brand-lockup" href="/" aria-label="Bibliothèque">
        <img class="brand-logo" src="/neolingua-logo.png" width="72" height="40" alt="" />
        <h1 class="app-title">Neolingua</h1>
      </a>
      ${extra}
    </header>
  `;
}

app.innerHTML = `
  <main class="jam jam-companion" id="companion-root" data-phase="lobby">
    ${headerHtml(`<p class="jam-code-inline" id="session-code">${escapeHtml(sessionId || "····")}</p>`)}

    <section class="jam-join" id="join-panel">
      <label class="field" id="join-code-field">
        <span>Code du jam</span>
        <input id="join-code" type="text" maxlength="8" autocomplete="off" placeholder="AB12" />
      </label>
      <label class="field">
        <span>Ton pseudo</span>
        <input id="join-name" type="text" maxlength="24" autocomplete="nickname" placeholder="Ex. Sophie" value="${escapeHtml(myName)}" required />
      </label>
      <button class="btn btn-primary" id="join-btn" type="button">Rejoindre</button>
      <p class="status" id="status">Entre le code et ton pseudo</p>
    </section>

    <section class="jam-companion-main" id="companion-main" hidden>
      <p class="jam-peers" id="peers">Connexion…</p>

      <div class="jam-lobby-ui" id="lobby-ui">
        <article class="jam-episode-card" id="episode-preview">
          <p class="jam-preview-kicker" id="preview-series">En attente de l’épisode…</p>
          <h2 class="jam-preview-title" id="preview-title"></h2>
          <p class="jam-preview-meta" id="preview-meta"></p>
        </article>
        <div class="jam-admin-panel" id="admin-panel" hidden>
          <button class="btn btn-primary jam-launch-btn" id="launch-btn" type="button">
            Lancer
          </button>
          <p class="jam-admin-note">Tu es l’admin · tout le monde te suit</p>
        </div>
        <p class="jam-waiting" id="waiting-launch" hidden>En attente du lancement…</p>
        <p class="status" id="session-status"></p>
      </div>

      <button class="btn jam-end-btn" id="end-jam-btn" type="button" hidden>
        Terminer le jam
      </button>

      <div class="jam-results" id="results-panel" hidden>
        <p class="jam-results-kicker" id="results-kicker">Jam terminé</p>
        <h2 class="jam-results-title">Classement</h2>
        <ol class="jam-leaderboard" id="leaderboard"></ol>
        <p class="jam-results-note">À points égaux, le plus rapide gagne</p>
      </div>

      <div class="jam-quiz-ui" id="quiz-ui">
        <p class="jam-quiz-timer" id="quiz-timer"></p>
        <p class="jam-quiz-prompt">Trouve le mot manquant</p>
        <p class="jam-quiz-sentence" id="quiz-sentence"></p>
        <div class="jam-quiz-options" id="quiz-options"></div>
        <button class="btn btn-primary jam-quiz-submit" id="quiz-submit" type="button" hidden>Valider</button>
        <div class="jam-quiz-feedback" id="quiz-feedback" hidden></div>
        <p class="jam-quiz-countdown" id="quiz-countdown" hidden></p>
        <div class="jam-quiz-wait" id="quiz-wait" hidden>
          <p class="jam-quiz-wait-label" id="quiz-wait-label"></p>
        </div>
      </div>

      <div class="jam-play-ui" id="play-ui">
        <div class="jam-subs" id="subtitle-stack">
          <p class="subtitle subtitle-en is-empty" id="subtitle-en"></p>
          <p class="subtitle subtitle-fr" id="subtitle-fr" hidden></p>
        </div>
        <div class="jam-sub-tracks" id="sub-tracks" aria-label="Sous-titres">
          <div class="jam-sub-row">
            <button class="sub-track-toggle" id="toggle-en" type="button" aria-pressed="true">English</button>
            <div class="jam-sub-size">
              <button class="btn-size" id="size-en-down" type="button" aria-label="Réduire English">A−</button>
              <button class="btn-size" id="size-en-up" type="button" aria-label="Agrandir English">A+</button>
            </div>
          </div>
          <div class="jam-sub-row">
            <button class="sub-track-toggle" id="toggle-fr" type="button" aria-pressed="true">Français</button>
            <div class="jam-sub-size">
              <button class="btn-size" id="size-fr-down" type="button" aria-label="Réduire Français">A−</button>
              <button class="btn-size" id="size-fr-up" type="button" aria-label="Agrandir Français">A+</button>
            </div>
          </div>
        </div>
        <div class="jam-remote-bar">
          <button class="jam-skip-intro" id="skip-intro" type="button" hidden>
            Passer le générique
          </button>
          <p class="timecode jam-companion-time" id="timecode">0:00 / 0:00</p>
          <div class="jam-companion-actions" id="companion-transport">
            <button class="btn-transport jam-big" id="back-5" type="button" aria-label="Reculer de 5 secondes">−5</button>
            <button class="btn-transport btn-play jam-big" id="play-pause" type="button" aria-label="Lecture" title="Lecture">
              <svg class="icon icon-play" viewBox="0 0 24 24" aria-hidden="true">
                <path d="M8 5v14l11-7z"/>
              </svg>
              <svg class="icon icon-pause" viewBox="0 0 24 24" aria-hidden="true">
                <path d="M7 5h3v14H7zm7 0h3v14h-3z"/>
              </svg>
            </button>
            <button class="btn-transport jam-big" id="fwd-5" type="button" aria-label="Avancer de 5 secondes">+5</button>
          </div>
        </div>
      </div>
    </section>

    <div class="jam-toast" id="toast" hidden></div>
  </main>
`;

const statusEl = document.querySelector("#status");
const sessionStatusEl = document.querySelector("#session-status");
const peersEl = must("#peers");
const toastEl = must("#toast");
const sessionCodeEl = document.querySelector("#session-code");
const timecodeEl = document.querySelector("#timecode");
const playPauseBtn = document.querySelector("#play-pause");
const back5Btn = document.querySelector("#back-5");
const fwd5Btn = document.querySelector("#fwd-5");
const skipIntroBtn = document.querySelector("#skip-intro");

back5Btn?.addEventListener("click", () => sendCommand("seekBy", { delta: -5 }));
fwd5Btn?.addEventListener("click", () => sendCommand("seekBy", { delta: 5 }));
playPauseBtn?.addEventListener("click", () => sendCommand("toggle"));
skipIntroBtn?.addEventListener("click", () => {
  if (introEndSeconds == null || jamPhase !== "playing") return;
  sendCommand("seek", { t: introEndSeconds });
});

void bootCompanion();

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") ensureJamSocket(role);
  if (document.visibilityState === "visible" && presenceTimer) {
    void pushPresence(jamPhase === "playing");
  }
});
window.addEventListener("pageshow", () => {
  ensureJamSocket(role);
});

async function bootCompanion() {
  const joinPanel = must("#join-panel");
  const companionMain = must("#companion-main");
  const launchBtn = document.querySelector("#launch-btn");
  const toggleEn = document.querySelector("#toggle-en");
  const toggleFr = document.querySelector("#toggle-fr");
  const sizeEnDown = document.querySelector("#size-en-down");
  const sizeEnUp = document.querySelector("#size-en-up");
  const sizeFrDown = document.querySelector("#size-fr-down");
  const sizeFrUp = document.querySelector("#size-fr-up");
  const subtitleEn = document.querySelector("#subtitle-en");
  const subtitleFr = document.querySelector("#subtitle-fr");
  const subtitleStack = document.querySelector("#subtitle-stack");

  applySubtitleSizes();

  if (toggleEn && toggleFr && subtitleEn && subtitleFr && subtitleStack) {
    syncCompanionSubControls(toggleEn, toggleFr, subtitleEn, subtitleFr);
    toggleEn.addEventListener("click", () => {
      showEn = !showEn;
      localStorage.setItem(SUB_EN_KEY, showEn ? "1" : "0");
      syncCompanionSubControls(toggleEn, toggleFr, subtitleEn, subtitleFr);
      renderCompanionCue(estimateTime());
    });
    toggleFr.addEventListener("click", () => {
      showFr = !showFr;
      localStorage.setItem(SUB_FR_KEY, showFr ? "1" : "0");
      syncCompanionSubControls(toggleEn, toggleFr, subtitleEn, subtitleFr);
      renderCompanionCue(estimateTime());
    });
  }

  sizeEnDown?.addEventListener("click", () => changeSubtitleSize("en", -1));
  sizeEnUp?.addEventListener("click", () => changeSubtitleSize("en", 1));
  sizeFrDown?.addEventListener("click", () => changeSubtitleSize("fr", -1));
  sizeFrUp?.addEventListener("click", () => changeSubtitleSize("fr", 1));

  launchBtn?.addEventListener("click", () => {
    if (!isAdmin || jamPhase !== "lobby") return;
    ensureAudio();
    ws?.send(JSON.stringify({ type: "launch", by: myName }));
  });

  const endJamBtn = document.querySelector("#end-jam-btn");
  endJamBtn?.addEventListener("click", () => {
    if (!isAdmin || (jamPhase !== "playing" && jamPhase !== "quiz")) return;
    if (!window.confirm("Terminer le jam et afficher le classement ?")) return;
    ws?.send(JSON.stringify({ type: "endJam", by: myName }));
  });

  const joinBtn = document.querySelector("#join-btn");
  const joinCodeInput = document.querySelector("#join-code");
  const joinNameInput = document.querySelector("#join-name");
  const joinCodeField = document.querySelector("#join-code-field");

  if (sessionId && joinCodeInput) {
    joinCodeInput.value = sessionId;
    joinCodeInput.readOnly = true;
    if (joinCodeField) joinCodeField.classList.add("is-locked");
    setStatus("Choisis un pseudo pour rejoindre", "idle");
  } else {
    setStatus("Entre le code et ton pseudo", "idle");
  }
  if (joinNameInput && myName) {
    joinNameInput.value = myName;
  }
  joinNameInput?.focus();

  const doJoin = async () => {
    const code = (joinCodeInput?.value || sessionId || "").trim().toUpperCase();
    if (!code) {
      setStatus("Entre un code", "error");
      joinCodeInput?.focus();
      return;
    }
    const name = (joinNameInput?.value || "").trim().slice(0, 24);
    if (!name) {
      setStatus("Choisis un pseudo", "error");
      joinNameInput?.focus();
      return;
    }
    myName = name;
    localStorage.setItem(NAME_KEY, myName);
    sessionId = code;
    if (sessionCodeEl) sessionCodeEl.textContent = sessionId;

    try {
      await api(`/api/jam/sessions/${encodeURIComponent(sessionId)}`);
    } catch {
      if (recalledPeerId()) {
        clearRecalledPeerId();
        notifyLateDisconnect("gone");
      } else {
        setStatus("Session introuvable", "error");
      }
      return;
    }

    joinPanel.hidden = true;
    companionMain.hidden = false;
    reconnectEnabled = true;
    setStatus("Connecté", "ok");
    await connectWs("companion");
    startPresenceLoop();
    startCompanionClockLoop();
    history.replaceState({}, "", `/jam.html?s=${encodeURIComponent(sessionId)}`);
  };

  joinBtn?.addEventListener("click", () => {
    void doJoin();
  });
  joinNameInput?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void doJoin();
    }
  });
  joinCodeInput?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      if ((joinNameInput?.value || "").trim()) void doJoin();
      else joinNameInput?.focus();
    }
  });

  updateCompanionPhaseUi();
}

function selectedEpisodeMeta() {
  return {
    path: selectedEpisode,
    title: selectedEpisode,
    seriesTitle: "",
    seasonLabel: "",
  };
}

function updateEpisodePreview(meta) {
  const seriesEl = document.querySelector("#preview-series");
  const titleEl = document.querySelector("#preview-title");
  const metaEl = document.querySelector("#preview-meta");
  if (seriesEl) seriesEl.textContent = meta.seriesTitle || "NeoLingua";
  if (titleEl) titleEl.textContent = meta.title || "Épisode";
  if (metaEl) metaEl.textContent = meta.seasonLabel || "";
  presenceTitle = [meta.seriesTitle, meta.title].filter(Boolean).join(" · ");
  presencePath = meta.path || "";
}

function updateCompanionPhaseUi() {
  if (role !== "companion") return;
  const root = document.querySelector("#companion-root");
  const adminPanel = document.querySelector("#admin-panel");
  const waiting = document.querySelector("#waiting-launch");
  const endJamBtn = document.querySelector("#end-jam-btn");
  const resultsPanel = document.querySelector("#results-panel");

  if (root) root.dataset.phase = jamPhase;

  const inLobby = jamPhase === "lobby" || jamPhase === "countdown";
  if (adminPanel) adminPanel.hidden = !(isAdmin && jamPhase === "lobby");
  if (endJamBtn) {
    endJamBtn.hidden = !(isAdmin && (jamPhase === "playing" || jamPhase === "quiz"));
  }
  if (resultsPanel) resultsPanel.hidden = jamPhase !== "ended";
  if (waiting) {
    waiting.hidden = !(inLobby && !isAdmin);
    if (jamPhase === "countdown") {
      waiting.hidden = false;
      waiting.textContent = "Compte à rebours…";
    } else {
      waiting.textContent = adminName
        ? `En attente que ${adminName} lance…`
        : "En attente d’un admin…";
    }
  }
}

function renderLeaderboard(entries, reason, by) {
  applyLeaderboard({
    boards: document.querySelectorAll("#leaderboard"),
    kickers: document.querySelectorAll("#results-kicker"),
    entries,
    reason,
    by,
    escapeHtml,
    highlightPeerId: myPeerId,
  });
}

function showJamEnded(msg) {
  clearQuizUiTimers();
  activeRoundId = null;
  jamPhase = "ended";
  renderLeaderboard(msg.leaderboard ?? [], msg.reason, msg.by);
  updateCompanionPhaseUi();
  stopPresenceLoop();

  if (role === "display") {
    const lobby = document.querySelector("#lobby");
    const sessionPanel = document.querySelector("#session-panel");
    const stage = document.querySelector("#stage");
    const results = document.querySelector("#results-panel");
    const countdown = document.querySelector("#countdown-overlay");
    const header = document.querySelector("#display-header");
    if (lobby) lobby.hidden = true;
    if (sessionPanel) sessionPanel.hidden = true;
    if (stage) {
      stage.hidden = true;
      stage.classList.remove("is-cinema");
    }
    if (countdown) countdown.hidden = true;
    if (header) header.hidden = false;
    if (results) results.hidden = false;
    displayPlayer?.pause();
  }
}

function clearQuizUiTimers() {
  if (quizTimerInterval != null) {
    window.clearInterval(quizTimerInterval);
    quizTimerInterval = null;
  }
  if (quizCountdownInterval != null) {
    window.clearInterval(quizCountdownInterval);
    quizCountdownInterval = null;
  }
}

/** Minimal quiz receive UI so protocol messages still paint if a host sends them. */
function startCompanionQuiz(msg, options = {}) {
  clearQuizUiTimers();
  if (!options.preserveSelections) {
    quizSelections = {};
    quizUsedOptionIndexes = new Set();
    quizGapOptionIndex = {};
    quizAnswered = false;
  }

  const sentence = document.querySelector("#quiz-sentence");
  const optionsEl = document.querySelector("#quiz-options");
  const feedback = document.querySelector("#quiz-feedback");
  const wait = document.querySelector("#quiz-wait");
  const countdown = document.querySelector("#quiz-countdown");
  const timer = document.querySelector("#quiz-timer");
  const submit = document.querySelector("#quiz-submit");
  if (!sentence || !optionsEl) return;

  if (feedback) {
    feedback.hidden = true;
    feedback.textContent = "";
  }
  if (wait) wait.hidden = true;
  if (countdown) {
    countdown.hidden = true;
    countdown.textContent = "";
  }
  if (submit) {
    submit.hidden = true;
    submit.disabled = false;
    submit.onclick = () => submitQuizAnswer(msg);
  }

  refreshQuizSentence(msg);
  optionsEl.innerHTML = "";
  (msg.options || []).forEach((word, index) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "jam-quiz-chip";
    btn.textContent = word;
    btn.dataset.optionIndex = String(index);
    btn.addEventListener("click", () => onQuizOptionPick(msg, word, index));
    optionsEl.appendChild(btn);
  });
  syncQuizOptionChips();

  const tick = () => {
    if (!timer) return;
    const left = Math.max(0, Math.ceil((msg.deadlineAt - Date.now()) / 1000));
    timer.textContent = `${left}s`;
    if (left <= 0 && quizTimerInterval != null) {
      window.clearInterval(quizTimerInterval);
      quizTimerInterval = null;
    }
  };
  tick();
  quizTimerInterval = window.setInterval(tick, 200);
}

function renderQuizSentence(segments, selections, locked) {
  return renderQuizSentenceHtml(segments, selections, locked, escapeHtml);
}

function refreshQuizSentence(msg) {
  const sentence = document.querySelector("#quiz-sentence");
  const submit = document.querySelector("#quiz-submit");
  if (!sentence) return;
  const gapIds = msg.gapIds || [];
  const filled = gapIds.every((id) => quizSelections[id]);
  sentence.innerHTML = renderQuizSentence(msg.segments, quizSelections, quizAnswered);
  if (!quizAnswered) {
    sentence.querySelectorAll(".jam-quiz-gap.is-removable").forEach((el) => {
      el.addEventListener("click", () => clearQuizGap(msg, el.getAttribute("data-gap")));
    });
  }
  if (submit) {
    submit.hidden = !(filled && !quizAnswered && gapIds.length > 0);
  }
}

function syncQuizOptionChips() {
  const optionsEl = document.querySelector("#quiz-options");
  if (!optionsEl) return;
  optionsEl.querySelectorAll(".jam-quiz-chip").forEach((btn) => {
    const index = Number(btn.dataset.optionIndex);
    const used = quizUsedOptionIndexes.has(index);
    btn.disabled = used || quizAnswered;
    btn.classList.toggle("is-used", used);
  });
}

function clearQuizGap(msg, gapId) {
  if (!gapId || quizAnswered) return;
  const prev = quizSelections[gapId];
  if (!prev) return;
  delete quizSelections[gapId];
  const optionIndex = quizGapOptionIndex[gapId];
  delete quizGapOptionIndex[gapId];
  if (optionIndex != null) quizUsedOptionIndexes.delete(optionIndex);
  refreshQuizSentence(msg);
  syncQuizOptionChips();
}

function onQuizOptionPick(msg, word, optionIndex) {
  if (quizAnswered || quizUsedOptionIndexes.has(optionIndex)) return;
  const gapIds = msg.gapIds || [];
  const next = gapIds.find((id) => !quizSelections[id]);
  if (!next) return;
  quizSelections[next] = word;
  quizGapOptionIndex[next] = optionIndex;
  quizUsedOptionIndexes.add(optionIndex);
  refreshQuizSentence(msg);
  syncQuizOptionChips();
}

function submitQuizAnswer(msg) {
  if (quizAnswered || !ws || ws.readyState !== WebSocket.OPEN) return;
  const gapIds = msg.gapIds || [];
  if (!gapIds.every((id) => quizSelections[id])) return;
  quizAnswered = true;
  refreshQuizSentence(msg);
  syncQuizOptionChips();
  ws.send(
    JSON.stringify({
      type: "quizAnswer",
      roundId: msg.roundId,
      answers: { ...quizSelections },
    }),
  );
}

function updateQuizProgress(answered, total) {
  const wait = document.querySelector("#quiz-wait");
  const waitLabel = document.querySelector("#quiz-wait-label");
  if (!wait || !waitLabel) return;
  wait.hidden = false;
  waitLabel.textContent = `Réponses ${answered}/${total}`;
}

function showQuizFeedback(msg) {
  const feedback = document.querySelector("#quiz-feedback");
  const wait = document.querySelector("#quiz-wait");
  if (wait) wait.hidden = !msg.waiting;
  if (!feedback) return;
  feedback.hidden = false;
  const rows = (msg.results || [])
    .map((r) => {
      const ok = r.correct;
      return `<p class="jam-quiz-result ${ok ? "is-correct" : "is-wrong"}">
        <span class="jam-quiz-mark">${ok ? "✓" : "✗"}</span>
        <span class="jam-quiz-result-body">
          <span class="jam-quiz-result-word">${escapeHtml(r.answer || "")}</span>
          ${
            !ok
              ? `<span class="jam-quiz-result-yours">Toi : ${escapeHtml(r.yours || "-")}</span>`
              : ""
          }
        </span>
      </p>`;
    })
    .join("");
  feedback.innerHTML = `<p class="jam-quiz-feedback-title">${msg.timedOut ? "Temps écoulé" : "Résultat"}</p>${rows}`;
}

function showQuizResumeCountdown(resumeAt) {
  const countdown = document.querySelector("#quiz-countdown");
  if (!countdown) return;
  countdown.hidden = false;
  clearQuizUiTimers();
  const tick = () => {
    const left = Math.max(0, Math.ceil((resumeAt - Date.now()) / 1000));
    countdown.textContent = left > 0 ? `Reprise dans ${left}s` : "";
    if (left <= 0 && quizCountdownInterval != null) {
      window.clearInterval(quizCountdownInterval);
      quizCountdownInterval = null;
      countdown.hidden = true;
    }
  };
  tick();
  quizCountdownInterval = window.setInterval(tick, 150);
}

async function prepareEpisodeInBackground(player) {
  const meta = selectedEpisodeMeta();
  videoReady = false;
  presenceTitle = [meta.seriesTitle, meta.title].filter(Boolean).join(" · ");
  presencePath = meta.path;
  setSessionStatus("Préparation de la vidéo…", "busy");

  try {
    await ensureReady(meta.path);
    player.src = `/api/video?path=${encodeURIComponent(meta.path)}`;
    player.load();
    videoReady = true;
    loadedEpisode = meta.path;

    ws?.send(
      JSON.stringify({
        type: "load",
        episodePath: meta.path,
        episodeTitle: meta.title,
        seriesTitle: meta.seriesTitle,
        seasonLabel: meta.seasonLabel,
        quizMode: false,
        quizIntervalSeconds: 60,
        by: myName,
      }),
    );
    setSessionStatus("Prêt · en attente du lancement admin", "ok");
  } catch (error) {
    const message = error instanceof Error ? error.message : "Préparation impossible";
    setSessionStatus(message, "error");
  }
}

function connectWs(connectRole) {
  if (!sessionId) return Promise.resolve();
  wsGeneration += 1;
  const generation = wsGeneration;
  ws?.close();

  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = `${proto}//${window.location.host}/ws/jam?session=${encodeURIComponent(sessionId)}`;

  return new Promise((resolve) => {
    const socket = new WebSocket(url);
    ws = socket;
    let settled = false;

    socket.addEventListener("open", () => {
      if (generation !== wsGeneration) return;
      socket.send(
        JSON.stringify({
          type: "hello",
          role: connectRole,
          name: myName,
          clientId: getJamClientId(),
        }),
      );
      setSessionStatus("Connecté", "ok");
      if (!settled) {
        settled = true;
        resolve();
      }
    });

    socket.addEventListener("message", (event) => {
      if (generation !== wsGeneration) return;
      let msg;
      try {
        msg = JSON.parse(String(event.data));
      } catch {
        return;
      }
      void handleServerMessage(msg);
    });

    socket.addEventListener("close", () => {
      if (generation !== wsGeneration) return;
      if (!reconnectEnabled) {
        setSessionStatus(
          "Déconnecté trop longtemps · la session n’est plus disponible",
          "error",
        );
        return;
      }
      setSessionStatus("Déconnecté · reconnexion…", "busy");
      window.setTimeout(() => {
        if (generation !== wsGeneration || !reconnectEnabled) return;
        void connectWs(connectRole);
      }, 800);
    });
  });
}

function ensureJamSocket(connectRole) {
  if (!sessionId || !reconnectEnabled) return;
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    return;
  }
  void connectWs(connectRole);
}

async function handleServerMessage(msg) {
  if (msg.type === "error") {
    if (msg.code === "session_gone") {
      reconnectEnabled = false;
      clearRecalledPeerId();
      notifyLateDisconnect("gone");
      stopPresenceLoop();
      return;
    }
    setSessionStatus(msg.message, "error");
    setStatus(msg.message, "error");
    return;
  }

  if (msg.type === "toast") {
    showToast(msg.message);
    return;
  }

  if (msg.type === "joined") {
    const previousPeer = myPeerId || recalledPeerId();
    const resumed = Boolean(msg.resumed);
    const lateRejoin = !resumed && Boolean(previousPeer) && previousPeer !== msg.peerId;
    myPeerId = msg.peerId;
    isAdmin = msg.isAdmin;
    rememberPeerId(msg.peerId);
    reconnectEnabled = true;
    updateCompanionPhaseUi();
    if (lateRejoin) notifyLateDisconnect("rejoined");
    return;
  }

  if (msg.type === "admin") {
    adminName = msg.adminName;
    isAdmin = Boolean(myPeerId && msg.adminId === myPeerId);
    updateCompanionPhaseUi();
    const hint = document.querySelector("#preview-hint");
    if (hint && role === "display") {
      hint.textContent = msg.adminName
        ? `Admin : ${msg.adminName} · en attente du lancement`
        : "En attente que quelqu’un rejoigne (premier = admin)";
    }
    return;
  }

  if (msg.type === "peers") {
    adminName = msg.adminName;
    isAdmin = Boolean(myPeerId && msg.adminId === myPeerId);
    companionCount = msg.companions;
    peersEl.textContent =
      msg.companions === 0
        ? "En attente de compagnons…"
        : `${msg.companions} compagnon${msg.companions > 1 ? "s" : ""} · ${msg.displays} écran${msg.displays > 1 ? "s" : ""}${
            msg.adminName ? ` · admin ${msg.adminName}` : ""
          }`;
    updateCompanionPhaseUi();
    return;
  }

  if (msg.type === "countdown") {
    jamPhase = "countdown";
    updateCompanionPhaseUi();
    if (role === "display") {
      void runDisplayCountdown(msg.endsAt);
    } else {
      setSessionStatus(`Compte à rebours · ${msg.by}`, "busy");
    }
    return;
  }

  if (msg.type === "go") {
    jamPhase = "playing";
    updateCompanionPhaseUi();
    syncPresencePlaying(true);
    if (role === "display") {
      await beginDisplayPlayback();
    }
    return;
  }

  if (msg.type === "state") {
    const wasQuiz = jamPhase === "quiz";
    const prevMode = quizModeEnabled;
    const prevInterval = quizIntervalSeconds;
    jamPhase = msg.phase;
    adminName = msg.adminName;
    isAdmin = Boolean(myPeerId && msg.adminId === myPeerId);
    if (typeof msg.quizMode === "boolean") quizModeEnabled = msg.quizMode;
    if (
      typeof msg.quizIntervalSeconds === "number" &&
      Number.isFinite(msg.quizIntervalSeconds)
    ) {
      quizIntervalSeconds = Math.max(5, Math.round(msg.quizIntervalSeconds));
    }
    applyClock(msg.t, msg.playing, msg.duration);
    if (msg.episodePath) {
      updateEpisodePreview({
        title: msg.episodeTitle || msg.episodePath,
        seriesTitle: msg.seriesTitle || "",
        seasonLabel: msg.seasonLabel || "",
        path: msg.episodePath,
      });
      if (role === "companion" && msg.episodePath !== loadedEpisode) {
        await loadCompanionEpisode(msg.episodePath);
      }
    }
    // Do not rebuild after quiz→playing: keep remaining spaced items (same as host).
    if (prevMode !== quizModeEnabled || prevInterval !== quizIntervalSeconds) {
      rebuildCompanionQuizSchedule(
        Math.max(QUIZ_FIRST_AFTER_SECONDS, Number(msg.t) || 0),
      );
    }
    if (!(role === "companion" && msg.phase === "quiz" && !wasQuiz)) {
      updateCompanionPhaseUi();
    }
    if (role === "companion") updatePlayIcon(msg.playing);
    syncPresencePlaying(msg.phase === "playing" && msg.playing);
    return;
  }

  if (msg.type === "quizStart") {
    const sameRound = jamPhase === "quiz" && activeRoundId === msg.roundId;
    jamPhase = "quiz";
    activeRoundId = msg.roundId;
    if (!sameRound) {
      quizSelections = {};
      quizUsedOptionIndexes = new Set();
      quizGapOptionIndex = {};
      quizAnswered = false;
    }
    if (role === "companion") {
      startCompanionQuiz(msg, { preserveSelections: sameRound });
      const subtitleEn = document.querySelector("#subtitle-en");
      const subtitleFr = document.querySelector("#subtitle-fr");
      if (subtitleEn) {
        subtitleEn.textContent = "";
        subtitleEn.classList.add("is-empty");
      }
      if (subtitleFr) {
        subtitleFr.textContent = "";
        subtitleFr.hidden = true;
      }
    }
    updateCompanionPhaseUi();
    return;
  }

  if (msg.type === "quizProgress") {
    if (role === "companion" && activeRoundId === msg.roundId) {
      updateQuizProgress(msg.answered, msg.total);
    }
    return;
  }

  if (msg.type === "quizFeedback") {
    if (role === "companion" && activeRoundId === msg.roundId) {
      showQuizFeedback(msg);
    }
    return;
  }

  if (msg.type === "quizComplete") {
    if (role === "companion" && activeRoundId === msg.roundId) {
      showQuizResumeCountdown(msg.resumeAt);
    }
    return;
  }

  if (msg.type === "quizEnded") {
    clearQuizUiTimers();
    activeRoundId = null;
    if (jamPhase !== "ended") jamPhase = "playing";
    updateCompanionPhaseUi();
    if (role === "companion") {
      restoreCompanionSubVisibility();
      renderCompanionCue(estimateTime());
    }
    return;
  }

  if (msg.type === "jamEnded") {
    showJamEnded(msg);
    return;
  }

  if (msg.type === "command") {
    applyClock(msg.t, msg.playing, clockDuration);
    if (role === "display") {
      await applyDisplayCommand(msg);
    } else {
      updatePlayIcon(msg.playing);
    }
  }
}

async function runDisplayCountdown(endsAt) {
  const lobby = document.querySelector("#lobby");
  const sessionPanel = document.querySelector("#session-panel");
  const stage = document.querySelector("#stage");
  const overlay = document.querySelector("#countdown-overlay");
  const numEl = document.querySelector("#countdown-num");
  const header = document.querySelector("#display-header");

  if (lobby) lobby.hidden = true;
  if (sessionPanel) sessionPanel.hidden = true;
  if (stage) {
    stage.hidden = true;
    stage.classList.remove("is-cinema");
  }
  if (header) header.hidden = true;
  if (overlay) overlay.hidden = false;

  ensureAudio();
  if (countdownTimer != null) window.clearInterval(countdownTimer);

  let lastShown = -1;
  const tick = () => {
    const left = Math.max(0, Math.ceil((endsAt - Date.now()) / 1000));
    if (numEl) numEl.textContent = String(left);
    if (left !== lastShown && left > 0) {
      playTick(left <= 2);
      lastShown = left;
    }
    if (left <= 0 && countdownTimer != null) {
      window.clearInterval(countdownTimer);
      countdownTimer = null;
    }
  };
  tick();
  countdownTimer = window.setInterval(tick, 100);
}

async function beginDisplayPlayback() {
  const overlay = document.querySelector("#countdown-overlay");
  const stage = document.querySelector("#stage");
  const sessionPanel = document.querySelector("#session-panel");
  const header = document.querySelector("#display-header");
  const player = displayPlayer;
  if (!player) return;

  if (overlay) overlay.hidden = true;
  if (sessionPanel) sessionPanel.hidden = true;
  if (header) header.hidden = true;
  if (stage) {
    stage.hidden = false;
    stage.classList.add("is-cinema");
  }

  if (!videoReady && selectedEpisode) {
    await prepareEpisodeInBackground(player);
  }

  applyingRemote = true;
  try {
    player.currentTime = 0;
    try {
      await player.play();
    } catch {
      setSessionStatus("Autoplay bloqué : appuie sur Play sur un téléphone", "error");
    }
  } finally {
    applyingRemote = false;
  }

  broadcastClock(player);
  syncPresencePlaying(true);
}

async function applyDisplayCommand(msg) {
  const player = displayPlayer;
  if (!player?.src) return;
  if (jamPhase !== "playing" && jamPhase !== "quiz") return;

  applyingRemote = true;
  try {
    if (msg.action === "seekBy" && typeof msg.delta === "number") {
      if (jamPhase === "quiz") return;
      player.currentTime = Math.max(
        0,
        Math.min(player.duration || Infinity, (player.currentTime || 0) + msg.delta),
      );
    } else if (msg.action === "seek") {
      if (jamPhase === "quiz") return;
      player.currentTime = msg.t;
    } else if (msg.action === "play" || (msg.action === "toggle" && msg.playing)) {
      try {
        await player.play();
      } catch {
        /* ignore */
      }
    } else if (msg.action === "pause" || (msg.action === "toggle" && !msg.playing)) {
      player.pause();
    }

    if (
      (msg.action === "play" || msg.action === "pause" || msg.action === "toggle") &&
      Math.abs((player.currentTime || 0) - msg.t) > 0.6
    ) {
      player.currentTime = msg.t;
    }

    if (jamPhase === "playing") broadcastClock(player);
  } finally {
    window.setTimeout(() => {
      applyingRemote = false;
    }, 50);
  }
}

async function loadCompanionEpisode(episodePath) {
  loadedEpisode = episodePath;
  cuesEn = [];
  cuesFr = [];
  hasEn = false;
  hasFr = false;
  introStartSeconds = null;
  introEndSeconds = null;
  updateSkipIntroVisibility(0);
  presencePath = episodePath;

  try {
    const status = await api(`/api/status?path=${encodeURIComponent(episodePath)}`);
    hasEn = status.subtitles?.en?.status === "ready";
    hasFr = status.subtitles?.fr?.status === "ready";
    if (hasEn) cuesEn = parseVtt(await fetchVtt(episodePath, "en"));
    if (hasFr) cuesFr = parseVtt(await fetchVtt(episodePath, "fr"));
    quizCues = cuesEn.map(annotatePlainCue);
    rebuildCompanionQuizSchedule(QUIZ_FIRST_AFTER_SECONDS);
    const range = detectIntroRangeSeconds(cuesEn.length ? cuesEn : cuesFr);
    introStartSeconds = range.start;
    introEndSeconds = range.end;
    setSessionStatus("Sous-titres prêts", "ok");
    updateSkipIntroVisibility(estimateTime());
  } catch (error) {
    setSessionStatus(error instanceof Error ? error.message : "Subs indisponibles", "error");
  }
}

function updateSkipIntroVisibility(time) {
  if (!skipIntroBtn) return;
  const visible =
    jamPhase === "playing" &&
    introStartSeconds != null &&
    introEndSeconds != null &&
    time >= introStartSeconds &&
    time < introEndSeconds - 0.25;
  skipIntroBtn.hidden = !visible;
}

function startCompanionClockLoop() {
  const loop = () => {
    if (jamPhase === "playing") {
      const t = estimateTime();
      renderCompanionCue(t);
      updateSkipIntroVisibility(t);
      if (timecodeEl) {
        timecodeEl.textContent = `${formatTime(t)} / ${formatTime(clockDuration)}`;
      }
      updatePlayIcon(clockPlaying);
    } else {
      updateSkipIntroVisibility(0);
    }
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
}

function applyClock(t, playing, duration) {
  clockT = t;
  clockPlaying = playing;
  if (duration > 0) clockDuration = duration;
  clockReceivedAt = performance.now();
}

function estimateTime() {
  const drift = clockPlaying ? (performance.now() - clockReceivedAt) / 1000 : 0;
  const t = clockT + drift;
  if (clockDuration > 0) return Math.min(clockDuration, Math.max(0, t));
  return Math.max(0, t);
}

function broadcastClock(player) {
  if (!ws || ws.readyState !== WebSocket.OPEN || jamPhase !== "playing") return;

  const sendOnce = () => {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(
      JSON.stringify({
        type: "clock",
        t: player.currentTime || 0,
        playing: !player.paused && !player.ended,
        duration: player.duration || 0,
      }),
    );
  };

  sendOnce();

  if (clockPollTimer != null) {
    window.clearInterval(clockPollTimer);
    clockPollTimer = null;
  }

  if (!player.paused && !player.ended) {
    clockPollTimer = window.setInterval(() => {
      if (player.paused || player.ended) {
        sendOnce();
        if (clockPollTimer != null) window.clearInterval(clockPollTimer);
        clockPollTimer = null;
        return;
      }
      sendOnce();
    }, 400);
  }
}

function sendCommand(action, extra = {}) {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  if (jamPhase !== "playing") return;
  ws.send(
    JSON.stringify({
      type: "command",
      action,
      delta: extra.delta,
      t: extra.t,
      by: myName,
    }),
  );
}

function updatePlayIcon(playing) {
  if (!playPauseBtn) return;
  playPauseBtn.classList.toggle("is-playing", playing);
  playPauseBtn.setAttribute("aria-label", playing ? "Pause" : "Lecture");
  playPauseBtn.title = playing ? "Pause" : "Lecture";
}

function syncCompanionSubControls(toggleEn, toggleFr, en, fr) {
  toggleEn.setAttribute("aria-pressed", showEn ? "true" : "false");
  toggleFr.setAttribute("aria-pressed", showFr ? "true" : "false");
  toggleEn.classList.toggle("is-on", showEn);
  toggleFr.classList.toggle("is-on", showFr);
  en.hidden = !showEn;
  fr.hidden = !showFr;

  const sizeEnDown = document.querySelector("#size-en-down");
  const sizeEnUp = document.querySelector("#size-en-up");
  const sizeFrDown = document.querySelector("#size-fr-down");
  const sizeFrUp = document.querySelector("#size-fr-up");
  if (sizeEnDown) sizeEnDown.disabled = sizeEnIndex <= 0;
  if (sizeEnUp) sizeEnUp.disabled = sizeEnIndex >= SIZE_STEPS.length - 1;
  if (sizeFrDown) sizeFrDown.disabled = sizeFrIndex <= 0;
  if (sizeFrUp) sizeFrUp.disabled = sizeFrIndex >= SIZE_STEPS.length - 1;
}

function restoreCompanionSubVisibility() {
  const toggleEn = document.querySelector("#toggle-en");
  const toggleFr = document.querySelector("#toggle-fr");
  const subtitleEn = document.querySelector("#subtitle-en");
  const subtitleFr = document.querySelector("#subtitle-fr");
  if (toggleEn && toggleFr && subtitleEn && subtitleFr) {
    syncCompanionSubControls(toggleEn, toggleFr, subtitleEn, subtitleFr);
  }
}

function changeSubtitleSize(lang, delta) {
  if (lang === "en") {
    sizeEnIndex = clampSizeIndex(sizeEnIndex + delta);
    localStorage.setItem(SUB_EN_SIZE_KEY, String(sizeEnIndex));
  } else {
    sizeFrIndex = clampSizeIndex(sizeFrIndex + delta);
    localStorage.setItem(SUB_FR_SIZE_KEY, String(sizeFrIndex));
  }
  applySubtitleSizes();
  restoreCompanionSubVisibility();
}

function applySubtitleSizes() {
  document.documentElement.style.setProperty("--sub-en-size", `${SIZE_STEPS[sizeEnIndex]}rem`);
  document.documentElement.style.setProperty("--sub-fr-size", `${SIZE_STEPS[sizeFrIndex]}rem`);
}

function renderCompanionCue(time) {
  if (jamPhase === "quiz") return;
  const subtitleEn = document.querySelector("#subtitle-en");
  const subtitleFr = document.querySelector("#subtitle-fr");
  if (!subtitleEn || !subtitleFr) return;

  const annotated =
    quizCues.find((cue) => time >= cue.start && time <= cue.end + 0.05) ?? null;
  const scheduled =
    quizModeEnabled && jamPhase === "playing" && annotated
      ? findScheduledQuiz(quizSchedule, annotated)
      : undefined;
  const quizLine = Boolean(scheduled);

  let showEnNow = showEn;
  let showFrNow = showFr;
  // Quiz blanks are on EN: hide FR so it does not spoil the answer.
  if (quizLine) showFrNow = false;

  subtitleEn.hidden = !showEnNow;
  subtitleFr.hidden = !showFrNow;

  if (showEnNow) {
    if (annotated && scheduled) {
      subtitleEn.innerHTML = renderGappedCueHtml(annotated, scheduled.gapTokenIndexes);
      subtitleEn.classList.toggle("is-empty", false);
    } else {
      const text = cueAt(cuesEn, time);
      setCueText(
        subtitleEn,
        text || (loadedEpisode ? (hasEn ? "…" : "EN manquant") : "…"),
        !text,
      );
    }
  }
  if (showFrNow) {
    const text = cueAt(cuesFr, time);
    setCueText(
      subtitleFr,
      text || (loadedEpisode ? (hasFr ? "…" : "FR manquant") : "…"),
      !text,
    );
  } else {
    subtitleFr.textContent = "";
    subtitleFr.classList.add("is-empty");
  }
}

function setCueText(el, text, isEmpty) {
  el.textContent = text;
  el.classList.toggle("is-empty", isEmpty);
}

function cueAt(cues, time) {
  const cue = cues.find((c) => time >= c.start && time < c.end);
  return cue?.text || "";
}

function ensureAudio() {
  if (audioCtx) return;
  const AC = window.AudioContext || window.webkitAudioContext;
  if (!AC) return;
  audioCtx = new AC();
}

function playTick(high = false) {
  ensureAudio();
  if (!audioCtx) return;
  const osc = audioCtx.createOscillator();
  const gain = audioCtx.createGain();
  osc.type = "square";
  osc.frequency.value = high ? 980 : 720;
  gain.gain.value = 0.05;
  osc.connect(gain);
  gain.connect(audioCtx.destination);
  const now = audioCtx.currentTime;
  osc.start(now);
  gain.gain.exponentialRampToValueAtTime(0.001, now + 0.07);
  osc.stop(now + 0.08);
}

async function buildJoinUrl(code) {
  const path = `/jam.html?s=${encodeURIComponent(code)}`;
  const host = window.location.hostname;
  const isLoopback = host === "localhost" || host === "127.0.0.1";

  if (!isLoopback) {
    return new URL(path, window.location.origin).toString();
  }

  try {
    const info = await api("/api/jam/info");
    const lan = info.addresses?.[0];
    if (lan) {
      const port = window.location.port;
      const portPart = port ? `:${port}` : "";
      return `${window.location.protocol}//${lan}${portPart}${path}`;
    }
  } catch {
    // fallback
  }

  return new URL(path, window.location.origin).toString();
}

function isPlaybackReady(status) {
  return status?.video?.status === "ready" && status?.subtitles?.status !== "missing";
}

async function ensureReady(path) {
  let status = await api(`/api/status?path=${encodeURIComponent(path)}`);
  if (isPlaybackReady(status)) return status;

  setSessionStatus("Démarrage de la préparation…", "busy");
  await api("/api/prepare-video", {
    method: "POST",
    body: JSON.stringify({ path }),
  });

  for (;;) {
    status = await api(`/api/status?path=${encodeURIComponent(path)}`);
    if (status.video?.status === "error") {
      throw new Error(
        (status.video.message && String(status.video.message).trim()) || "Échec de préparation",
      );
    }
    if (isPlaybackReady(status)) return status;
    const pct = typeof status.video?.progress === "number" ? status.video.progress : null;
    const message =
      status.video?.message ||
      (pct == null ? "Préparation en cours…" : `Préparation ${pct} %`);
    setSessionStatus(message, "busy");
    await sleep(700);
  }
}

async function fetchVtt(episodePath, lang) {
  const res = await fetch(
    `/api/subtitles.vtt?path=${encodeURIComponent(episodePath)}&lang=${lang}`,
  );
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.text();
}

function showToast(message, durationMs = 1800) {
  toastEl.hidden = false;
  toastEl.textContent = message;
  if (toastTimer != null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toastEl.hidden = true;
  }, durationMs);
}

function setStatus(message, tone) {
  if (!statusEl) return;
  statusEl.hidden = !message;
  statusEl.textContent = message;
  statusEl.dataset.tone = tone || "idle";
}

function setSessionStatus(message, tone) {
  if (!sessionStatusEl) return;
  sessionStatusEl.textContent = message || "";
  sessionStatusEl.dataset.tone = tone || "idle";
}

async function api(path, options) {
  const res = await fetch(path, {
    headers: { "Content-Type": "application/json", ...(options?.headers || {}) },
    ...options,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.error || data.message || `HTTP ${res.status}`);
  }
  return data;
}

function must(selector) {
  const el = document.querySelector(selector);
  if (!el) throw new Error(`Missing ${selector}`);
  return el;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function pushPresence(playing) {
  const body = {
    clientId: viewerClientId(),
    mode: "jam",
    playing: Boolean(playing),
  };
  if (playing && presencePath) {
    body.title = presenceTitle || undefined;
    body.path = presencePath;
  }
  const payload = JSON.stringify(body);
  try {
    await api("/api/presence", {
      method: "POST",
      body: payload,
      keepalive: true,
    });
  } catch {
    try {
      if (typeof navigator !== "undefined" && typeof navigator.sendBeacon === "function") {
        const blob = new Blob([payload], { type: "application/json" });
        navigator.sendBeacon("/api/presence", blob);
      }
    } catch {
      /* ignore */
    }
  }
}

function startPresenceLoop() {
  if (presenceTimer) {
    clearInterval(presenceTimer);
    presenceTimer = null;
  }
  void pushPresence(jamPhase === "playing");
  presenceTimer = setInterval(() => {
    if (document.visibilityState !== "hidden") {
      void pushPresence(jamPhase === "playing");
    }
  }, PRESENCE_INTERVAL_MS);
}

function stopPresenceLoop() {
  const wasActive = presenceTimer != null;
  if (presenceTimer) {
    clearInterval(presenceTimer);
    presenceTimer = null;
  }
  if (wasActive) void pushPresence(false);
}

function syncPresencePlaying(playing) {
  if (!presenceTimer) return;
  void pushPresence(playing);
}

// Silence unused companionCount warning in some tooling; kept for parity with POC peers UI.
void companionCount;
