/**
 * NeoLingua Center Jam Host (ES module).
 * Runs a Jam DISPLAY session inside the Solo SPA.
 */

import {
  annotatePlainCue,
  buildQuizSchedule,
  findScheduledQuiz,
  normalizeQuizInterval,
  QUIZ_FIRST_AFTER_SECONDS,
  QUIZ_INTERVAL_OPTIONS,
  renderGappedCueHtml,
} from "./jam-gaps.js";
import { applyLeaderboard } from "./leaderboard.js";
import {
  SIZE_STEPS,
  clampSizeIndex,
  escapeHtml,
  formatClock,
  parseVtt,
} from "./media-utils.js";
import { createChromeController } from "./player-chrome.js";
import { PRESENCE_INTERVAL_MS, viewerClientId } from "./presence-client.js";

const CLIENT_ID_KEY = "neolingua.jam.clientId.display";
const DISPLAY_SUB_EN_KEY = "neolingua.jam.display.showEn";
const DISPLAY_SUB_FR_KEY = "neolingua.jam.display.showFr";
const SUB_EN_SIZE_KEY = "neolingua.jam.display.sizeEn";
const SUB_FR_SIZE_KEY = "neolingua.jam.display.sizeFr";

/**
 * Mount a Jam host (display) session.
 * @param {{
 *   root: HTMLElement,
 *   path: string,
 *   title: string,
 *   seriesTitle?: string,
 *   seasonLabel?: string,
 *   onCancel?: () => void,
 * }} opts
 * @returns {Promise<{ destroy: () => void }>}
 */
export async function mountJamHost(opts) {
  const { root, path, title, seriesTitle = "", seasonLabel = "", onCancel } = opts;

  // State
  let sessionId = "";
  let ws = null;
  let reconnectEnabled = true;
  let jamPhase = "lobby";
  let clockDuration = 0;
  let clockPollTimer = null;
  let countdownTimer = null;
  let applyingRemote = false;
  let displayPlayer = null;
  let wsGeneration = 0;
  let videoReady = false;
  let presenceTimer = null;
  let presenceTitle = "";
  let presencePath = "";
  let audioCtx = null;
  let quizModeEnabled = false;
  let quizIntervalSeconds = 60;
  /** @type {{ start: number, end: number, text: string, tokens: object[] }[]} */
  let quizCues = [];
  /** @type {{ cueStart: number, cueEnd: number, proposal: object }[]} */
  let quizSchedule = [];
  /** @type {{ start: number, end: number, text: string, tokens: object[] } | null} */
  let watchingCue = null;
  let companionCount = 0;
  /** Subtitles on the TV / display (in addition to companions). */
  let displayShowEn = false;
  let displayShowFr = false;
  let sizeEnIndex = clampSizeIndex(Number(localStorage.getItem(SUB_EN_SIZE_KEY) ?? "3"));
  let sizeFrIndex = clampSizeIndex(Number(localStorage.getItem(SUB_FR_SIZE_KEY) ?? "2"));
  let scrubbing = false;
  /** @type {{ start: number, end: number, text: string }[]} */
  let cuesEnPlain = [];
  /** @type {{ start: number, end: number, text: string }[]} */
  let cuesFrPlain = [];

  // DOM setup
  root.innerHTML = `
    <section class="jam-lobby" id="jam-lobby">
      <article class="jam-preview" id="jam-setup-preview">
        <p class="jam-preview-kicker">${escapeHtml(seriesTitle || "NeoLingua")}</p>
        <h2 class="jam-preview-title">${escapeHtml(title || "Épisode")}</h2>
        <p class="jam-preview-meta">${escapeHtml(seasonLabel || "")}</p>
      </article>
      <div class="jam-setup-options" id="jam-setup-options">
        <label class="jam-setup-row">
          <input type="checkbox" id="jam-quiz-mode" />
          <span>Mode Quiz (trous de vocabulaire)</span>
        </label>
        <label class="jam-setup-field" id="jam-quiz-freq-wrap">
          <span>Fréquence des quiz</span>
          <select id="jam-quiz-interval" aria-label="Fréquence des quiz">
            ${QUIZ_INTERVAL_OPTIONS.map(
              (opt) =>
                `<option value="${opt.seconds}">${escapeHtml(opt.label)}</option>`,
            ).join("")}
          </select>
        </label>
        <fieldset class="jam-setup-fieldset">
          <legend>Sous-titres sur l'écran vidéo</legend>
          <p class="jam-setup-note">En plus du téléphone. Sur une phrase quiz, l'autre langue est masquée.</p>
          <label class="jam-setup-row">
            <input type="checkbox" id="jam-display-sub-en" />
            <span>Anglais</span>
          </label>
          <label class="jam-setup-row">
            <input type="checkbox" id="jam-display-sub-fr" />
            <span>Français</span>
          </label>
        </fieldset>
        <p class="jam-setup-note">Les options viennent des réglages Center. Tu peux les changer pour ce Jam seulement.</p>
        <div class="jam-setup-actions">
          <button class="btn btn-ghost" id="jam-cancel-setup" type="button">Annuler</button>
          <button class="btn btn-primary" id="jam-create-btn" type="button">Créer le salon</button>
        </div>
      </div>
      <p class="status" id="jam-status" hidden></p>
    </section>

    <section class="jam-session" id="jam-session-panel" hidden>
      <div class="jam-invite">
        <div class="jam-qr-wrap">
          <canvas id="jam-qr" width="200" height="200" aria-label="QR code pour rejoindre"></canvas>
        </div>
        <div class="jam-invite-copy">
          <p class="jam-code-label">Code</p>
          <p class="jam-code" id="jam-session-code">····</p>
          <p class="jam-join-url" id="jam-join-url"></p>
          <p class="jam-peers" id="jam-peers">En attente de compagnons…</p>
          <p class="jam-quiz-summary" id="jam-quiz-summary"></p>
          <p class="status" id="jam-session-status"></p>
        </div>
      </div>
      <article class="jam-preview" id="jam-episode-preview">
        <p class="jam-preview-kicker" id="jam-preview-series"></p>
        <h2 class="jam-preview-title" id="jam-preview-title"></h2>
        <p class="jam-preview-meta" id="jam-preview-meta"></p>
      </article>
      <article class="jam-preview jam-preview--hint">
        <p class="jam-preview-hint" id="jam-preview-hint">En attente que l'admin lance depuis son téléphone</p>
      </article>
      <p class="jam-back-catalog">
        <button type="button" class="btn btn-ghost" id="jam-cancel-session">Annuler</button>
      </p>
    </section>

    <section class="jam-results" id="jam-results-panel" hidden>
      <p class="jam-results-kicker" id="jam-results-kicker">Jam terminé</p>
      <h2 class="jam-results-title">Classement</h2>
      <ol class="jam-leaderboard" id="jam-leaderboard"></ol>
      <p class="jam-results-note">À points égaux, le plus rapide gagne</p>
    </section>

    <div class="jam-countdown" id="jam-countdown-overlay" hidden>
      <p class="jam-countdown-num" id="jam-countdown-num">5</p>
      <p class="jam-countdown-label">Ça commence</p>
    </div>

    <section class="stage" id="jam-stage" hidden>
      <div class="player-shell" id="jam-player-shell">
        <div class="player-frame" id="jam-video-wrap">
          <video id="jam-player" playsinline></video>
          <div class="subs" id="jam-display-subs" hidden>
            <p class="sub-line is-empty" id="jam-display-cue-en"></p>
            <p class="sub-line is-empty" id="jam-display-cue-fr" hidden></p>
          </div>
        </div>
        <div class="player-chrome" id="jam-player-chrome">
          <div class="transport">
            <input
              class="scrubber"
              id="jam-scrubber"
              type="range"
              min="0"
              max="0"
              step="0.1"
              value="0"
              aria-label="Position"
            />
            <div class="transport-row">
              <p class="timecode" id="jam-timecode">0:00 / 0:00</p>
              <div class="transport-actions">
                <button class="btn-transport" id="jam-to-start" type="button" aria-label="Revenir au début" title="Début">
                  <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M6 5h2v14H6zm3.5 7l9.5 6.5V5.5z"/>
                  </svg>
                </button>
                <button class="btn-transport" id="jam-back-5" type="button" aria-label="Reculer de 5 secondes" title="−5 s">
                  <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M12 5V2L7 6.5 12 11V8a5 5 0 1 1-4.9 6H5.08A7 7 0 1 0 12 5z"/>
                    <text x="12" y="15.2" text-anchor="middle" class="icon-num">5</text>
                  </svg>
                </button>
                <button class="btn-transport btn-play" id="jam-play-pause" type="button" aria-label="Lecture" title="Lecture">
                  <svg class="icon icon-play" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M8 5v14l11-7z"/>
                  </svg>
                  <svg class="icon icon-pause" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M7 5h3v14H7zm7 0h3v14h-3z"/>
                  </svg>
                </button>
                <button class="btn-transport" id="jam-fwd-5" type="button" aria-label="Avancer de 5 secondes" title="+5 s">
                  <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M12 5V2l5 4.5L12 11V8a5 5 0 1 0 4.9 6h2.02A7 7 0 1 1 12 5z"/>
                    <text x="12" y="15.2" text-anchor="middle" class="icon-num">5</text>
                  </svg>
                </button>
              </div>
              <button type="button" id="jam-toggle-fullscreen" class="btn-fullscreen" title="Plein écran">
                Plein écran
              </button>
            </div>
          </div>
          <div class="player-toolbar" aria-label="Sous-titres">
            <div class="sub-lang-control">
              <button type="button" class="btn-sub-size" id="jam-size-en-down" aria-label="Réduire les sous-titres anglais" title="Réduire EN">−</button>
              <button type="button" id="jam-toggle-en" class="" aria-pressed="false">EN</button>
              <button type="button" class="btn-sub-size" id="jam-size-en-up" aria-label="Agrandir les sous-titres anglais" title="Agrandir EN">+</button>
            </div>
            <div class="sub-lang-control">
              <button type="button" class="btn-sub-size" id="jam-size-fr-down" aria-label="Réduire les sous-titres français" title="Réduire FR">−</button>
              <button type="button" id="jam-toggle-fr" class="" aria-pressed="false">FR</button>
              <button type="button" class="btn-sub-size" id="jam-size-fr-up" aria-label="Agrandir les sous-titres français" title="Agrandir FR">+</button>
            </div>
          </div>
        </div>
      </div>
    </section>

    <div class="jam-toast" id="jam-toast" hidden></div>
  `;

  const lobbyEl = must(root, "#jam-lobby");
  const sessionPanel = must(root, "#jam-session-panel");
  const stage = must(root, "#jam-stage");
  const player = must(root, "#jam-player");
  const statusEl = must(root, "#jam-status");
  const sessionStatusEl = must(root, "#jam-session-status");
  const peersEl = must(root, "#jam-peers");
  const toastEl = must(root, "#jam-toast");
  const sessionCodeEl = must(root, "#jam-session-code");
  const joinUrlEl = must(root, "#jam-join-url");
  const qrCanvas = must(root, "#jam-qr");
  const countdownOverlay = must(root, "#jam-countdown-overlay");
  const previewSeriesEl = must(root, "#jam-preview-series");
  const previewTitleEl = must(root, "#jam-preview-title");
  const previewMetaEl = must(root, "#jam-preview-meta");
  const previewHintEl = must(root, "#jam-preview-hint");
  const resultsPanel = must(root, "#jam-results-panel");
  const leaderboardEl = must(root, "#jam-leaderboard");
  const resultsKickerEl = must(root, "#jam-results-kicker");
  const quizModeInput = must(root, "#jam-quiz-mode");
  const quizIntervalSelect = must(root, "#jam-quiz-interval");
  const quizFreqWrap = must(root, "#jam-quiz-freq-wrap");
  const displaySubEnInput = must(root, "#jam-display-sub-en");
  const displaySubFrInput = must(root, "#jam-display-sub-fr");
  const displaySubsEl = must(root, "#jam-display-subs");
  const displaySubEnEl = must(root, "#jam-display-cue-en");
  const displaySubFrEl = must(root, "#jam-display-cue-fr");
  const playerShell = must(root, "#jam-player-shell");
  const chrome = createChromeController({
    getShell: () => playerShell,
    shouldHide: () => {
      if (!isDocumentFullscreen() || jamPhase !== "playing") return false;
      if (!displayPlayer || displayPlayer.paused || displayPlayer.ended || scrubbing) {
        return false;
      }
      return true;
    },
  });
  const scheduleChromeHide = () => chrome.schedule();
  /** @param {boolean} [keepVisible] */
  const revealChrome = (keepVisible = false) => chrome.reveal(keepVisible);
  const clearChromeHideTimer = () => chrome.clear();
  const scrubber = must(root, "#jam-scrubber");
  const timecodeEl = must(root, "#jam-timecode");
  const playPauseBtn = must(root, "#jam-play-pause");
  const toStartBtn = must(root, "#jam-to-start");
  const back5Btn = must(root, "#jam-back-5");
  const fwd5Btn = must(root, "#jam-fwd-5");
  const toggleFsBtn = must(root, "#jam-toggle-fullscreen");
  const playerChrome = must(root, "#jam-player-chrome");
  const toggleEnBtn = must(root, "#jam-toggle-en");
  const toggleFrBtn = must(root, "#jam-toggle-fr");
  const sizeEnDown = must(root, "#jam-size-en-down");
  const sizeEnUp = must(root, "#jam-size-en-up");
  const sizeFrDown = must(root, "#jam-size-fr-down");
  const sizeFrUp = must(root, "#jam-size-fr-up");
  const createBtn = must(root, "#jam-create-btn");
  const setupOptions = must(root, "#jam-setup-options");
  const quizSummaryEl = must(root, "#jam-quiz-summary");

  displayPlayer = player;
  displaySubEnInput.checked = displayShowEn;
  displaySubFrInput.checked = displayShowFr;
  presenceTitle = [seriesTitle, title].filter(Boolean).join(" · ");
  presencePath = path;

  // Helper functions
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

  function must(container, selector) {
    const el = container.querySelector(selector);
    if (!el) throw new Error(`Missing ${selector}`);
    return el;
  }

  async function api(apiPath, options) {
    const res = await fetch(apiPath, {
      headers: { "Content-Type": "application/json", ...options?.headers },
      ...options,
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) {
      throw new Error(data.error || data.message || `HTTP ${res.status}`);
    }
    return data;
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function setStatus(message, tone) {
    statusEl.hidden = !message;
    statusEl.textContent = message;
    statusEl.dataset.tone = tone || "idle";
  }

  function setSessionStatus(message, tone) {
    sessionStatusEl.textContent = message || "";
    sessionStatusEl.dataset.tone = tone || "idle";
  }

  function showToast(message, durationMs = 1800) {
    toastEl.hidden = false;
    toastEl.textContent = message;
    setTimeout(() => {
      toastEl.hidden = true;
    }, durationMs);
  }

  function updateEpisodePreview(meta) {
    previewSeriesEl.textContent = meta.seriesTitle || "NeoLingua";
    previewTitleEl.textContent = meta.title || "Épisode";
    previewMetaEl.textContent = meta.seasonLabel || "";
    presenceTitle = [meta.seriesTitle, meta.title].filter(Boolean).join(" · ");
    presencePath = meta.path || "";
  }

  function renderLeaderboard(entries, reason, by) {
    applyLeaderboard({
      boards: [leaderboardEl],
      kickers: [resultsKickerEl],
      entries,
      reason,
      by,
      escapeHtml,
    });
  }

  function showJamEnded(msg) {
    jamPhase = "ended";
    renderLeaderboard(msg.leaderboard ?? [], msg.reason, msg.by);
    stopPresenceLoop();

    lobbyEl.hidden = true;
    sessionPanel.hidden = true;
    stage.hidden = true;
    stage.classList.remove("is-cinema");
    countdownOverlay.hidden = true;
    resultsPanel.hidden = false;
    displayPlayer?.pause();
  }

  async function buildJoinUrl(code) {
    const joinPath = `/jam.html?s=${encodeURIComponent(code)}`;
    const host = window.location.hostname;
    const isLoopback = host === "localhost" || host === "127.0.0.1";

    if (!isLoopback) {
      return new URL(joinPath, window.location.origin).toString();
    }

    try {
      const info = await api("/api/jam/info");
      const lan = info.addresses?.[0];
      if (lan) {
        const port = window.location.port;
        const portPart = port ? `:${port}` : "";
        return `${window.location.protocol}//${lan}${portPart}${joinPath}`;
      }
    } catch {
      // fallback
    }

    return new URL(joinPath, window.location.origin).toString();
  }

  function isPlaybackReady(status) {
    return status?.video?.status === "ready" && status?.subtitles?.status !== "missing";
  }

  async function ensureReady(episodePath) {
    let status = await api(`/api/status?path=${encodeURIComponent(episodePath)}`);
    if (isPlaybackReady(status)) return status;

    setSessionStatus("Démarrage de la préparation…", "busy");
    await api("/api/prepare-video", {
      method: "POST",
      body: JSON.stringify({ path: episodePath }),
    });

    for (;;) {
      status = await api(`/api/status?path=${encodeURIComponent(episodePath)}`);
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

  function rebuildQuizSchedule(firstAfterSeconds = QUIZ_FIRST_AFTER_SECONDS) {
    if (!quizModeEnabled || quizCues.length === 0) {
      quizSchedule = [];
      return;
    }
    quizSchedule = buildQuizSchedule(quizCues, quizIntervalSeconds, firstAfterSeconds);
  }

  function pruneQuizScheduleBefore(time) {
    quizSchedule = quizSchedule.filter((item) => item.cueEnd >= time - 0.25);
  }

  function consumeScheduledQuiz(cue) {
    const key = Math.round(cue.start * 1000);
    const index = quizSchedule.findIndex(
      (item) => Math.round(item.cueStart * 1000) === key,
    );
    if (index < 0) return null;
    const [item] = quizSchedule.splice(index, 1);
    return item ?? null;
  }

  function maybeProposeQuiz(videoPlayer) {
    if (!quizModeEnabled || quizCues.length === 0) return;
    if (jamPhase !== "playing") return;
    if (companionCount < 1) return;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    if (videoPlayer.paused || videoPlayer.ended || videoPlayer.seeking) return;

    const t = videoPlayer.currentTime || 0;
    const current =
      quizCues.find((cue) => t >= cue.start && t <= cue.end + 0.05) ?? null;

    if (watchingCue && (!current || current.start !== watchingCue.start)) {
      const ended = watchingCue;
      watchingCue = current;
      // Line must be fully behind the playhead (avoids quiz after seek / skip)
      if (ended.end > t + 0.2) return;

      const scheduled = consumeScheduledQuiz(ended);
      if (!scheduled) return;

      const proposal = scheduled.proposal;
      videoPlayer.pause();
      ws.send(
        JSON.stringify({
          type: "quizPropose",
          t,
          segments: proposal.segments,
          gaps: proposal.gaps.map((g) => ({
            id: g.id,
            answer: g.answer,
            level: g.level,
          })),
          options: proposal.options,
        }),
      );
      return;
    }

    watchingCue = current;
  }

  function cueTextAt(cues, time) {
    const cue = cues.find((c) => time >= c.start && time < c.end);
    return cue?.text || "";
  }

  function clearDisplaySubs() {
    displaySubEnEl.textContent = "";
    displaySubEnEl.classList.add("is-empty");
    displaySubFrEl.textContent = "";
    displaySubFrEl.classList.add("is-empty");
    displaySubsEl.hidden = true;
  }

  function renderDisplaySubs(time) {
    if (jamPhase === "quiz" || jamPhase === "countdown" || jamPhase === "ended") {
      clearDisplaySubs();
      return;
    }
    if (!displayShowEn && !displayShowFr) {
      clearDisplaySubs();
      return;
    }

    const annotated =
      quizCues.find((cue) => time >= cue.start && time <= cue.end + 0.05) ?? null;
    const scheduled =
      quizModeEnabled && jamPhase === "playing" && annotated
        ? findScheduledQuiz(quizSchedule, annotated)
        : undefined;
    const quizLine = Boolean(scheduled);

    let showEnNow = displayShowEn;
    let showFrNow = displayShowFr;
    // Quiz blanks are on EN: never show FR on that line (would spoil the answer).
    if (quizLine) showFrNow = false;

    displaySubsEl.hidden = !(showEnNow || showFrNow);
    displaySubFrEl.hidden = !showFrNow;

    if (showEnNow) {
      if (annotated && scheduled) {
        displaySubEnEl.innerHTML = renderGappedCueHtml(
          annotated,
          scheduled.gapTokenIndexes,
        );
        displaySubEnEl.classList.toggle("is-empty", false);
      } else if (annotated) {
        displaySubEnEl.textContent = annotated.text;
        displaySubEnEl.classList.toggle("is-empty", !annotated.text);
      } else {
        const text = cueTextAt(cuesEnPlain, time);
        displaySubEnEl.textContent = text;
        displaySubEnEl.classList.toggle("is-empty", !text);
      }
    } else {
      displaySubEnEl.textContent = "";
      displaySubEnEl.classList.add("is-empty");
    }

    if (showFrNow) {
      const text = cueTextAt(cuesFrPlain, time);
      displaySubFrEl.textContent = text;
      displaySubFrEl.classList.toggle("is-empty", !text);
    } else {
      displaySubFrEl.textContent = "";
      displaySubFrEl.classList.add("is-empty");
    }
  }

  function applyDisplaySubtitleSizes() {
    playerShell.style.setProperty("--sub-en-size", `${SIZE_STEPS[sizeEnIndex]}rem`);
    playerShell.style.setProperty("--sub-fr-size", `${SIZE_STEPS[sizeFrIndex]}rem`);
    sizeEnDown.disabled = sizeEnIndex <= 0;
    sizeEnUp.disabled = sizeEnIndex >= SIZE_STEPS.length - 1;
    sizeFrDown.disabled = sizeFrIndex <= 0;
    sizeFrUp.disabled = sizeFrIndex >= SIZE_STEPS.length - 1;
  }

  function syncDisplaySubToggles() {
    toggleEnBtn.classList.toggle("is-active", displayShowEn);
    toggleEnBtn.setAttribute("aria-pressed", displayShowEn ? "true" : "false");
    toggleFrBtn.classList.toggle("is-active", displayShowFr);
    toggleFrBtn.setAttribute("aria-pressed", displayShowFr ? "true" : "false");
    displaySubFrEl.hidden = !displayShowFr;
  }

  function updatePlayIcon(playing) {
    playPauseBtn.classList.toggle("is-playing", playing);
    playPauseBtn.setAttribute("aria-label", playing ? "Pause" : "Lecture");
    playPauseBtn.title = playing ? "Pause" : "Lecture";
  }

  function syncTransportUi(videoPlayer) {
    const t = videoPlayer.currentTime || 0;
    const d = Number.isFinite(videoPlayer.duration) ? videoPlayer.duration : 0;
    timecodeEl.textContent = `${formatClock(t)} / ${formatClock(d)}`;
    if (!scrubbing) {
      scrubber.max = String(d || 0);
      scrubber.value = String(t);
    }
    updatePlayIcon(!videoPlayer.paused && !videoPlayer.ended);
  }

  function sendDisplayCommand(action, extra = {}) {
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    if (jamPhase !== "playing") return;
    ws.send(
      JSON.stringify({
        type: "command",
        action,
        delta: extra.delta,
        t: extra.t,
        by: "Écran",
      }),
    );
  }

  function isDocumentFullscreen() {
    return Boolean(document.fullscreenElement || document.webkitFullscreenElement);
  }

  async function toggleSessionFullscreen() {
    if (isDocumentFullscreen()) {
      try {
        if (document.exitFullscreen) await document.exitFullscreen();
        else if (document.webkitExitFullscreen) document.webkitExitFullscreen();
      } catch {
        // ignore
      }
      return;
    }
    await enterSessionFullscreen();
  }

  function bindPlayerChrome() {
    applyDisplaySubtitleSizes();
    syncDisplaySubToggles();

    playPauseBtn.addEventListener("click", () => {
      if (jamPhase !== "playing" || !displayPlayer) return;
      sendDisplayCommand("toggle");
    });
    toStartBtn.addEventListener("click", () => {
      if (jamPhase !== "playing" || !displayPlayer) return;
      sendDisplayCommand("seek", { t: 0 });
    });
    back5Btn.addEventListener("click", () => {
      if (jamPhase !== "playing" || !displayPlayer) return;
      sendDisplayCommand("seekBy", { delta: -5 });
    });
    fwd5Btn.addEventListener("click", () => {
      if (jamPhase !== "playing" || !displayPlayer) return;
      sendDisplayCommand("seekBy", { delta: 5 });
    });
    toggleFsBtn.addEventListener("click", () => {
      void toggleSessionFullscreen();
    });

    scrubber.addEventListener("pointerdown", () => {
      scrubbing = true;
      revealChrome(true);
    });
    scrubber.addEventListener("pointerup", () => {
      scrubbing = false;
      scheduleChromeHide();
    });
    scrubber.addEventListener("change", () => {
      if (jamPhase !== "playing" || !displayPlayer) return;
      const t = Number(scrubber.value);
      if (!Number.isFinite(t)) return;
      sendDisplayCommand("seek", { t });
    });
    scrubber.addEventListener("input", () => {
      const t = Number(scrubber.value);
      const d = Number(scrubber.max) || 0;
      timecodeEl.textContent = `${formatClock(t)} / ${formatClock(d)}`;
    });

    toggleEnBtn.addEventListener("click", () => {
      displayShowEn = !displayShowEn;
      displaySubEnInput.checked = displayShowEn;
      localStorage.setItem(DISPLAY_SUB_EN_KEY, displayShowEn ? "1" : "0");
      syncDisplaySubToggles();
      if (displayPlayer) renderDisplaySubs(displayPlayer.currentTime || 0);
      revealChrome();
    });
    toggleFrBtn.addEventListener("click", () => {
      displayShowFr = !displayShowFr;
      displaySubFrInput.checked = displayShowFr;
      localStorage.setItem(DISPLAY_SUB_FR_KEY, displayShowFr ? "1" : "0");
      syncDisplaySubToggles();
      if (displayPlayer) renderDisplaySubs(displayPlayer.currentTime || 0);
      revealChrome();
    });

    sizeEnDown.addEventListener("click", () => {
      sizeEnIndex = clampSizeIndex(sizeEnIndex - 1);
      localStorage.setItem(SUB_EN_SIZE_KEY, String(sizeEnIndex));
      applyDisplaySubtitleSizes();
      revealChrome();
    });
    sizeEnUp.addEventListener("click", () => {
      sizeEnIndex = clampSizeIndex(sizeEnIndex + 1);
      localStorage.setItem(SUB_EN_SIZE_KEY, String(sizeEnIndex));
      applyDisplaySubtitleSizes();
      revealChrome();
    });
    sizeFrDown.addEventListener("click", () => {
      sizeFrIndex = clampSizeIndex(sizeFrIndex - 1);
      localStorage.setItem(SUB_FR_SIZE_KEY, String(sizeFrIndex));
      applyDisplaySubtitleSizes();
      revealChrome();
    });
    sizeFrUp.addEventListener("click", () => {
      sizeFrIndex = clampSizeIndex(sizeFrIndex + 1);
      localStorage.setItem(SUB_FR_SIZE_KEY, String(sizeFrIndex));
      applyDisplaySubtitleSizes();
      revealChrome();
    });

    playerShell.addEventListener("mousemove", () => {
      if (jamPhase === "playing") revealChrome();
    });
    playerShell.addEventListener("pointerdown", () => {
      if (jamPhase === "playing") revealChrome();
    });
    playerChrome.addEventListener("mouseenter", () => {
      clearChromeHideTimer();
      playerShell.classList.remove("is-chrome-hidden");
    });
    playerChrome.addEventListener("mouseleave", () => {
      scheduleChromeHide();
    });

    const onFsChange = () => {
      toggleFsBtn.textContent = isDocumentFullscreen() ? "Fenêtré" : "Plein écran";
      toggleFsBtn.title = toggleFsBtn.textContent;
      revealChrome(!isDocumentFullscreen());
    };
    document.addEventListener("fullscreenchange", onFsChange);
    document.addEventListener("webkitfullscreenchange", onFsChange);
    onFsChange();
    return () => {
      clearChromeHideTimer();
      document.removeEventListener("fullscreenchange", onFsChange);
      document.removeEventListener("webkitfullscreenchange", onFsChange);
    };
  }

  async function loadQuizCuesFromSubtitles() {
    cuesEnPlain = [];
    cuesFrPlain = [];
    quizCues = [];
    try {
      const enRes = await fetch(
        `/api/subtitles.vtt?path=${encodeURIComponent(path)}&lang=en`,
      );
      if (enRes.ok) {
        const text = await enRes.text();
        cuesEnPlain = parseVtt(text);
        quizCues = cuesEnPlain.map(annotatePlainCue);
      }
    } catch {
      /* ignore */
    }
    try {
      const frRes = await fetch(
        `/api/subtitles.vtt?path=${encodeURIComponent(path)}&lang=fr`,
      );
      if (frRes.ok) {
        const text = await frRes.text();
        cuesFrPlain = parseVtt(text);
      }
    } catch {
      /* ignore */
    }
  }

  async function prepareEpisodeInBackground() {
    videoReady = false;
    setSessionStatus("Préparation de la vidéo…", "busy");

    try {
      await ensureReady(path);
      player.src = `/api/video?path=${encodeURIComponent(path)}`;
      player.load();
      await loadQuizCuesFromSubtitles();
      rebuildQuizSchedule(QUIZ_FIRST_AFTER_SECONDS);
      videoReady = true;

      ws?.send(
        JSON.stringify({
          type: "load",
          episodePath: path,
          episodeTitle: title,
          seriesTitle,
          seasonLabel,
          quizMode: quizModeEnabled,
          quizIntervalSeconds,
          by: "",
        }),
      );
      setSessionStatus(
        quizModeEnabled && quizCues.length === 0
          ? "Prêt · quiz sans sous-titres EN (quiz désactivé pour cet épisode)"
          : "Prêt · en attente du lancement admin",
        "ok",
      );
    } catch (error) {
      const message = error instanceof Error ? error.message : "Préparation impossible";
      setSessionStatus(message, "error");
    }
  }

  function broadcastClock(videoPlayer) {
    if (!ws || ws.readyState !== WebSocket.OPEN || jamPhase !== "playing") return;

    const sendOnce = () => {
      if (!ws || ws.readyState !== WebSocket.OPEN) return;
      ws.send(
        JSON.stringify({
          type: "clock",
          t: videoPlayer.currentTime || 0,
          playing: !videoPlayer.paused && !videoPlayer.ended,
          duration: videoPlayer.duration || 0,
        }),
      );
    };

    sendOnce();

    if (clockPollTimer != null) {
      clearInterval(clockPollTimer);
      clockPollTimer = null;
    }

    if (!videoPlayer.paused && !videoPlayer.ended) {
      clockPollTimer = setInterval(() => {
        if (videoPlayer.paused || videoPlayer.ended) {
          sendOnce();
          if (clockPollTimer != null) clearInterval(clockPollTimer);
          clockPollTimer = null;
          return;
        }
        sendOnce();
      }, 400);
    }
  }

  function applyClock(_t, _playing, duration) {
    if (duration > 0) clockDuration = duration;
  }

  async function applyDisplayCommand(msg) {
    const videoPlayer = displayPlayer;
    if (!videoPlayer?.src) return;
    if (jamPhase !== "playing" && jamPhase !== "quiz") return;

    applyingRemote = true;
    try {
      if (msg.action === "seekBy" && typeof msg.delta === "number") {
        if (jamPhase === "quiz") return;
        videoPlayer.currentTime = Math.max(
          0,
          Math.min(videoPlayer.duration || Infinity, (videoPlayer.currentTime || 0) + msg.delta),
        );
      } else if (msg.action === "seek") {
        if (jamPhase === "quiz") return;
        videoPlayer.currentTime = msg.t;
      } else if (msg.action === "play" || (msg.action === "toggle" && msg.playing)) {
        try {
          await videoPlayer.play();
        } catch {
          /* ignore */
        }
      } else if (msg.action === "pause" || (msg.action === "toggle" && !msg.playing)) {
        videoPlayer.pause();
      }

      if (
        (msg.action === "play" || msg.action === "pause" || msg.action === "toggle") &&
        Math.abs((videoPlayer.currentTime || 0) - msg.t) > 0.6
      ) {
        videoPlayer.currentTime = msg.t;
      }

      if (jamPhase === "playing") {
        broadcastClock(videoPlayer);
        syncTransportUi(videoPlayer);
        renderDisplaySubs(videoPlayer.currentTime || 0);
      }
    } finally {
      setTimeout(() => {
        applyingRemote = false;
      }, 50);
    }
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

  async function enterSessionFullscreen() {
    const already = Boolean(
      document.fullscreenElement || document.webkitFullscreenElement,
    );
    if (already) return;
    const target = document.documentElement;
    try {
      if (typeof target.requestFullscreen === "function") {
        await target.requestFullscreen();
      } else if (typeof target.webkitRequestFullscreen === "function") {
        target.webkitRequestFullscreen();
      }
    } catch {
      // Browser may block without a user gesture.
    }
  }

  async function runDisplayCountdown(endsAt) {
    lobbyEl.hidden = true;
    sessionPanel.hidden = true;
    stage.hidden = true;
    stage.classList.remove("is-cinema");
    countdownOverlay.hidden = false;

    const numEl = must(root, "#jam-countdown-num");

    ensureAudio();
    if (countdownTimer != null) clearInterval(countdownTimer);

    let lastShown = -1;
    const tick = () => {
      const left = Math.max(0, Math.ceil((endsAt - Date.now()) / 1000));
      numEl.textContent = String(left);
      if (left !== lastShown && left > 0) {
        playTick(left <= 2);
        lastShown = left;
      }
      if (left <= 0 && countdownTimer != null) {
        clearInterval(countdownTimer);
        countdownTimer = null;
      }
    };
    tick();
    countdownTimer = setInterval(tick, 100);
  }

  async function beginDisplayPlayback() {
    const videoPlayer = displayPlayer;
    if (!videoPlayer) return;

    countdownOverlay.hidden = true;
    sessionPanel.hidden = true;
    stage.hidden = false;
    stage.classList.add("is-cinema");

    if (!videoReady) {
      await prepareEpisodeInBackground();
    }

    applyingRemote = true;
    try {
      videoPlayer.currentTime = 0;
      try {
        await videoPlayer.play();
      } catch {
        setSessionStatus("Autoplay bloqué : appuie sur Play sur un téléphone", "error");
      }
    } finally {
      applyingRemote = false;
    }

    broadcastClock(videoPlayer);
    syncTransportUi(videoPlayer);
    syncPresencePlaying(true);
    renderDisplaySubs(videoPlayer.currentTime || 0);
    scheduleChromeHide();
  }

  function connectWs() {
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
            role: "display",
            name: "",
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
            "Déconnecté trop longtemps · la session n'est plus disponible",
            "error",
          );
          return;
        }
        setSessionStatus("Déconnecté · reconnexion…", "busy");
        setTimeout(() => {
          if (generation !== wsGeneration || !reconnectEnabled) return;
          void connectWs();
        }, 800);
      });
    });
  }

  async function handleServerMessage(msg) {
    if (msg.type === "error") {
      if (msg.code === "session_gone") {
        reconnectEnabled = false;
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

    if (msg.type === "peers") {
      companionCount = Number(msg.companions) || 0;
      peersEl.textContent =
        msg.companions === 0
          ? "En attente de compagnons…"
          : `${msg.companions} compagnon${msg.companions > 1 ? "s" : ""} · ${msg.displays} écran${msg.displays > 1 ? "s" : ""}${
              msg.adminName ? ` · admin ${msg.adminName}` : ""
            }`;
      return;
    }

    if (msg.type === "admin") {
      previewHintEl.textContent = msg.adminName
        ? `Admin : ${msg.adminName} · en attente du lancement`
        : "En attente que quelqu'un rejoigne (premier = admin)";
      return;
    }

    if (msg.type === "countdown") {
      jamPhase = "countdown";
      await runDisplayCountdown(msg.endsAt);
      return;
    }

    if (msg.type === "go") {
      jamPhase = "playing";
      watchingCue = null;
      rebuildQuizSchedule(QUIZ_FIRST_AFTER_SECONDS);
      syncPresencePlaying(true);
      await beginDisplayPlayback();
      return;
    }

    if (msg.type === "state") {
      const prevPhase = jamPhase;
      jamPhase = msg.phase;
      if (typeof msg.quizMode === "boolean") {
        quizModeEnabled = msg.quizMode;
      }
      if (
        typeof msg.quizIntervalSeconds === "number" &&
        Number.isFinite(msg.quizIntervalSeconds)
      ) {
        quizIntervalSeconds = normalizeQuizInterval(msg.quizIntervalSeconds);
      }
      // Rebuild when entering play from lobby/countdown. Do not rebuild after a
      // quiz: that would make the next cue (~2s later) eligible immediately and
      // ignore the configured interval. Remaining schedule items stay spaced.
      if (msg.phase === "playing" && prevPhase !== "playing" && prevPhase !== "quiz") {
        rebuildQuizSchedule(
          Math.max(QUIZ_FIRST_AFTER_SECONDS, Number(msg.t) || 0),
        );
      }
      applyClock(msg.t, msg.playing, msg.duration);
      syncPresencePlaying(msg.phase === "playing" && msg.playing);
      return;
    }

    if (msg.type === "quizStart") {
      jamPhase = "quiz";
      displayPlayer?.pause();
      clearDisplaySubs();
      syncPresencePlaying(false);
      return;
    }

    if (msg.type === "quizEnded") {
      if (jamPhase !== "ended") jamPhase = "playing";
      watchingCue = null;
      if (displayPlayer) renderDisplaySubs(displayPlayer.currentTime || 0);
      return;
    }

    if (msg.type === "jamEnded") {
      showJamEnded(msg);
      return;
    }

    if (msg.type === "command") {
      applyClock(msg.t, msg.playing, clockDuration);
      await applyDisplayCommand(msg);
    }
  }

  function presenceIsLive() {
    // Treat setup + lobby as occupied so Center keeps episode context (like Solo watch).
    return jamPhase === "playing" || jamPhase === "quiz" || jamPhase === "countdown" || Boolean(presencePath);
  }

  async function pushPresence(playing) {
    const live = Boolean(playing);
    const body = {
      clientId: viewerClientId(),
      mode: "jam",
      playing: live,
    };
    if (live && presencePath) {
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
    void pushPresence(presenceIsLive());
    presenceTimer = setInterval(() => {
      if (document.visibilityState !== "hidden") {
        void pushPresence(presenceIsLive());
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

  function syncPresencePlaying(_playing) {
    if (!presenceTimer) return;
    void pushPresence(presenceIsLive());
  }

  // Video event handlers
  player.addEventListener("timeupdate", () => {
    syncTransportUi(player);
    if (applyingRemote || jamPhase !== "playing") return;
    broadcastClock(player);
    maybeProposeQuiz(player);
    renderDisplaySubs(player.currentTime || 0);
  });

  player.addEventListener("play", () => {
    updatePlayIcon(true);
    if (!applyingRemote && jamPhase === "playing") broadcastClock(player);
    syncPresencePlaying(true);
    if (jamPhase === "playing") scheduleChromeHide();
  });

  player.addEventListener("pause", () => {
    updatePlayIcon(false);
    if (!applyingRemote && jamPhase === "playing") broadcastClock(player);
    syncPresencePlaying(false);
    revealChrome(true);
  });

  player.addEventListener("seeking", () => {
    watchingCue = null;
  });

  player.addEventListener("seeked", () => {
    watchingCue = null;
    pruneQuizScheduleBefore(player.currentTime || 0);
    if (!applyingRemote && jamPhase === "playing") broadcastClock(player);
  });

  player.addEventListener("ended", () => {
    if (jamPhase !== "playing" && jamPhase !== "quiz") return;
    revealChrome(true);
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    ws.send(JSON.stringify({ type: "videoEnded" }));
  });

  // Visibility handling
  const handleVisibilityChange = () => {
    if (document.visibilityState === "visible") {
      if (ws && (ws.readyState === WebSocket.CLOSED || ws.readyState === WebSocket.CLOSING)) {
        void connectWs();
      }
      if (presenceTimer) {
        void pushPresence(jamPhase === "playing");
      }
    }
  };

  document.addEventListener("visibilitychange", handleVisibilityChange);

  function syncQuizFormEnabled() {
    const enabled = Boolean(quizModeInput.checked);
    quizIntervalSelect.disabled = !enabled;
    quizFreqWrap.classList.toggle("is-disabled", !enabled);
  }

  function readQuizForm() {
    quizModeEnabled = Boolean(quizModeInput.checked);
    quizIntervalSeconds = normalizeQuizInterval(quizIntervalSelect.value);
    displayShowEn = Boolean(displaySubEnInput.checked);
    displayShowFr = Boolean(displaySubFrInput.checked);
    localStorage.setItem(DISPLAY_SUB_EN_KEY, displayShowEn ? "1" : "0");
    localStorage.setItem(DISPLAY_SUB_FR_KEY, displayShowFr ? "1" : "0");
    syncDisplaySubToggles();
  }

  function quizSummaryText() {
    const subs = [];
    if (displayShowEn) subs.push("EN");
    if (displayShowFr) subs.push("FR");
    const subPart =
      subs.length === 0
        ? "écran sans sous-titres"
        : `écran ${subs.join("+")}`;
    if (!quizModeEnabled) return `Quiz désactivé · ${subPart}`;
    const opt = QUIZ_INTERVAL_OPTIONS.find((item) => item.seconds === quizIntervalSeconds);
    return `Quiz · toutes les ${opt?.label ?? `${quizIntervalSeconds}s`} · ${subPart}`;
  }

  async function createSalon() {
    // Fullscreen must run on this click (user gesture). Remote launch cannot.
    // Staying fullscreen through QR → countdown → video is the supported path.
    await enterSessionFullscreen();

    readQuizForm();
    createBtn.disabled = true;
    setupOptions.hidden = true;
    statusEl.hidden = false;
    setStatus("Création du jam…", "busy");
    try {
      const created = await api("/api/jam/sessions", {
        method: "POST",
        body: JSON.stringify({
          quizMode: quizModeEnabled,
          quizIntervalSeconds,
        }),
      });
      sessionId = created.sessionId;
      reconnectEnabled = true;
      sessionCodeEl.textContent = sessionId;
      quizSummaryEl.textContent = quizSummaryText();

      const joinUrl = await buildJoinUrl(sessionId);
      joinUrlEl.textContent = joinUrl;
      if (typeof QRCode !== "undefined" && typeof QRCode.toCanvas === "function") {
        await QRCode.toCanvas(qrCanvas, joinUrl, {
          width: 200,
          margin: 1,
          color: { dark: "#1c1917", light: "#ffffff" },
        });
      }

      updateEpisodePreview({ title, seriesTitle, seasonLabel, path });
      await connectWs();
      lobbyEl.hidden = true;
      sessionPanel.hidden = false;
      stage.hidden = true;
      stage.classList.remove("is-cinema");
      countdownOverlay.hidden = true;

      startPresenceLoop();
      void prepareEpisodeInBackground();
      setSessionStatus("Partage le QR · le premier téléphone devient admin", "ok");
    } catch (error) {
      createBtn.disabled = false;
      setupOptions.hidden = false;
      setStatus(error instanceof Error ? error.message : "Impossible de créer le jam", "error");
      if (document.fullscreenElement || document.webkitFullscreenElement) {
        try {
          if (document.exitFullscreen) await document.exitFullscreen();
          else if (document.webkitExitFullscreen) document.webkitExitFullscreen();
        } catch {
          // ignore
        }
      }
    }
  }

  // Prefill from host defaults (Center options), then allow per-Jam override.
  try {
    const settings = await api("/api/settings").catch(() => ({}));
    quizModeEnabled = Boolean(settings.jamQuizMode);
    quizIntervalSeconds = normalizeQuizInterval(settings.jamQuizIntervalSeconds);
    displayShowEn = Boolean(settings.jamDisplaySubEn);
    displayShowFr = Boolean(settings.jamDisplaySubFr);
  } catch {
    quizModeEnabled = false;
    quizIntervalSeconds = 60;
    displayShowEn = localStorage.getItem(DISPLAY_SUB_EN_KEY) === "1";
    displayShowFr = localStorage.getItem(DISPLAY_SUB_FR_KEY) === "1";
  }
  quizModeInput.checked = quizModeEnabled;
  quizIntervalSelect.value = String(quizIntervalSeconds);
  displaySubEnInput.checked = displayShowEn;
  displaySubFrInput.checked = displayShowFr;
  syncQuizFormEnabled();
  const unbindFullscreenUi = bindPlayerChrome();
  quizModeInput.addEventListener("change", syncQuizFormEnabled);
  createBtn.addEventListener("click", () => {
    void createSalon();
  });

  function goHome() {
    if (typeof onCancel === "function") {
      onCancel();
      return;
    }
    window.location.assign("/");
  }

  root.querySelector("#jam-cancel-setup")?.addEventListener("click", goHome);
  root.querySelector("#jam-cancel-session")?.addEventListener("click", goHome);

  // Announce the chosen episode immediately (setup screen), not only after play.
  startPresenceLoop();

  // Destroy function
  return {
    destroy: () => {
      reconnectEnabled = false;
      ws?.close();
      ws = null;
      if (clockPollTimer) clearInterval(clockPollTimer);
      if (countdownTimer) clearInterval(countdownTimer);
      stopPresenceLoop();
      displayPlayer?.pause();
      unbindFullscreenUi();
      document.removeEventListener("visibilitychange", handleVisibilityChange);
      root.innerHTML = "";
    },
  };
}
