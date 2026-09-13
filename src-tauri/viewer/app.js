import { mountJamHost } from "./jam-host.js";
import {
  SIZE_STEPS,
  clampSizeIndex,
  detectIntroRangeSeconds,
  escapeHtml,
  formatTime,
  parseVtt,
} from "./media-utils.js";
import { createChromeController } from "./player-chrome.js";
import { PRESENCE_INTERVAL_MS, viewerClientId } from "./presence-client.js";

const main = document.getElementById("main");

/** @typedef {{ id: string, title: string, displayTitle?: string|null, posterUrl?: string|null, tmdbId?: number|null, seasons: Season[] }} Series */
/** @typedef {{ number: number, title?: string|null, posterUrl?: string|null, episodes: Episode[] }} Season */
/** @typedef {{ id: string, season: number, episode: number, label: string, path: string, title?: string|null, prep?: Prep }} Episode */
/** @typedef {{ id: string, title: string, displayTitle?: string|null, path: string, posterUrl?: string|null, year?: number|null, tmdbId?: number|null }} Movie */
/** @typedef {{ status: string, video?: boolean, progress?: number, message?: string }} Prep */

/** @type {{ series: Series[], movies: Movie[] } | null} */
let catalog = null;
/**
 * @typedef {{ page: 'home' }} NavHome
 * @typedef {{ page: 'series', seriesId: string }} NavSeries
 * @typedef {{ page: 'season', seriesId: string, season: number }} NavSeason
 * @typedef {{ page: 'watch', kind: 'episode', seriesId: string, season: number, episode: number, path: string, title: string } | { page: 'watch', kind: 'movie', movieId: string, path: string, title: string }} NavWatch
 * @typedef {{ page: 'not-found', message: string }} NavNotFound
 * @typedef {NavHome | NavSeries | NavSeason | NavWatch | NavNotFound} Nav
 */
/** @type {Nav} */
let nav = { page: "home" };

/** @type {Map<string, Series>} */
let seriesBySlug = new Map();
/** @type {Map<string, string>} */
let seriesSlugById = new Map();
/** @type {Map<string, Movie>} */
let movieBySlug = new Map();
/** @type {Map<string, string>} */
let movieSlugById = new Map();

/** @type {Cue[]} */
let cuesEn = [];
/** @type {Cue[]} */
let cuesFr = [];
let showEn = true;
let showFr = false;

const SUB_EN_SIZE_KEY = "neolingua.viewer.sub.sizeEn";
const SUB_FR_SIZE_KEY = "neolingua.viewer.sub.sizeFr";

let sizeEnIndex = clampSizeIndex(Number(localStorage.getItem(SUB_EN_SIZE_KEY) ?? "3"));
let sizeFrIndex = clampSizeIndex(Number(localStorage.getItem(SUB_FR_SIZE_KEY) ?? "2"));

/** Host setting from Center: auto-skip intro when possible. */
let autoSkipIntro = false;
/** Host setting: delete remux/subs cache after playback ends. */
let purgeCacheAfterWatch = false;
/** @type {number | null} */
let introStartSeconds = null;
/** @type {number | null} */
let introEndSeconds = null;
/** Auto-skip runs at most once per watch; manual seek into intro stays allowed. */
let introAutoSkipped = false;
/** Request fullscreen on next watch boot (user gesture). */
let pendingFullscreen = false;
let scrubbing = false;
/** @type {HTMLElement | null} */
let chromeShell = null;
/** @type {HTMLVideoElement | null} */
let chromePlayer = null;

const chrome = createChromeController({
  getShell: () => chromeShell,
  shouldHide: () => {
    if (!chromeShell || !chromePlayer) return false;
    if (!isPlayerFullscreen(chromeShell)) return false;
    if (chromePlayer.paused || chromePlayer.ended || scrubbing) return false;
    return true;
  },
});

const WATCH_MODE_KEY = "neolingua.viewer.watchMode";
/** @type {ReturnType<typeof setInterval> | null} */
let presenceTimer = null;
/** @type {"solo" | "jam"} */
let watchMode = readWatchMode();
/** @type {{ destroy: () => void } | null} */
let jamHost = null;

function destroyJamHost() {
  if (!jamHost) return;
  try {
    jamHost.destroy();
  } catch {
    // Ignore teardown errors.
  }
  jamHost = null;
}

/**
 * @returns {"solo" | "jam"}
 */
function readWatchMode() {
  const raw = localStorage.getItem(WATCH_MODE_KEY);
  return raw === "jam" ? "jam" : "solo";
}

/**
 * @param {"solo" | "jam"} mode
 */
function setWatchMode(mode) {
  watchMode = mode === "jam" ? "jam" : "solo";
  localStorage.setItem(WATCH_MODE_KEY, watchMode);
}

/** Apply ?mode=jam|solo from the current URL (Center deep-links). */
function applyWatchModeFromLocation() {
  try {
    const mode = new URLSearchParams(window.location.search).get("mode");
    if (mode === "jam" || mode === "solo") setWatchMode(mode);
  } catch {
    /* ignore */
  }
}

/**
 * @typedef {{ start: number, end: number, text: string }} Cue
 */

const CONNECTION_PROBE_OK_MS = 12_000;
const CONNECTION_BACKOFF_START_MS = 1_000;
const CONNECTION_BACKOFF_MANUAL_MS = 10_000;
const CONNECTION_BACKOFF_MAX_MS = 30_000;

let connectionOnline = true;
let connectionBackoffMs = CONNECTION_BACKOFF_START_MS;
let connectionChecking = false;
/** @type {ReturnType<typeof setTimeout> | null} */
let connectionRetryTimer = null;
/** @type {ReturnType<typeof setInterval> | null} */
let connectionCountdownTimer = null;
/** @type {ReturnType<typeof setTimeout> | null} */
let connectionProbeTimer = null;
let connectionNextRetryAt = 0;
let connectionMonitorStarted = false;
let bootInFlight = false;

function connectionBannerEl() {
  return document.getElementById("connection-banner");
}

function connectionCountdownEl() {
  return document.getElementById("connection-banner-countdown");
}

function connectionRetryBtn() {
  return document.getElementById("connection-banner-retry");
}

function clearConnectionTimers() {
  if (connectionRetryTimer) {
    clearTimeout(connectionRetryTimer);
    connectionRetryTimer = null;
  }
  if (connectionCountdownTimer) {
    clearInterval(connectionCountdownTimer);
    connectionCountdownTimer = null;
  }
  if (connectionProbeTimer) {
    clearTimeout(connectionProbeTimer);
    connectionProbeTimer = null;
  }
}

function formatRetryCountdown(msLeft) {
  const seconds = Math.max(1, Math.ceil(msLeft / 1000));
  return seconds === 1
    ? "Nouvelle tentative dans 1 s…"
    : `Nouvelle tentative dans ${seconds} s…`;
}

function updateConnectionCountdown() {
  const el = connectionCountdownEl();
  if (!el) return;
  if (connectionChecking) {
    el.textContent = "Tentative en cours…";
    return;
  }
  const msLeft = connectionNextRetryAt - Date.now();
  if (msLeft <= 0) {
    el.textContent = "Nouvelle tentative sous peu…";
    return;
  }
  el.textContent = formatRetryCountdown(msLeft);
}

function connectionOverlayEl() {
  return document.getElementById("connection-overlay");
}

function setAppInteractionLocked(locked) {
  const app = document.querySelector(".app");
  if (app) app.inert = locked;
  const overlay = connectionOverlayEl();
  if (!overlay) return;
  overlay.hidden = !locked;
  overlay.setAttribute("aria-hidden", locked ? "false" : "true");
}

function showConnectionBanner() {
  const banner = connectionBannerEl();
  if (!banner) return;
  banner.hidden = false;
  document.body.classList.add("has-connection-banner");
  setAppInteractionLocked(true);
  updateConnectionCountdown();
}

function hideConnectionBanner() {
  const banner = connectionBannerEl();
  if (banner) banner.hidden = true;
  document.body.classList.remove("has-connection-banner");
  setAppInteractionLocked(false);
  const countdown = connectionCountdownEl();
  if (countdown) countdown.textContent = "Nouvelle tentative sous peu…";
  const retry = connectionRetryBtn();
  if (retry) retry.disabled = false;
}

function scheduleOnlineProbe() {
  if (connectionProbeTimer) {
    clearTimeout(connectionProbeTimer);
    connectionProbeTimer = null;
  }
  if (!connectionOnline) return;
  connectionProbeTimer = setTimeout(() => {
    connectionProbeTimer = null;
    void probeConnection();
  }, CONNECTION_PROBE_OK_MS);
}

function scheduleOfflineRetry() {
  if (connectionRetryTimer) {
    clearTimeout(connectionRetryTimer);
    connectionRetryTimer = null;
  }
  connectionNextRetryAt = Date.now() + connectionBackoffMs;
  updateConnectionCountdown();
  if (connectionCountdownTimer) {
    clearInterval(connectionCountdownTimer);
  }
  connectionCountdownTimer = setInterval(updateConnectionCountdown, 250);
  const delay = connectionBackoffMs;
  connectionRetryTimer = setTimeout(() => {
    connectionRetryTimer = null;
    void probeConnection();
  }, delay);
  connectionBackoffMs = Math.min(
    CONNECTION_BACKOFF_MAX_MS,
    Math.max(CONNECTION_BACKOFF_START_MS, connectionBackoffMs * 2),
  );
}

async function onConnectionRestored() {
  if (catalog != null || bootInFlight) return;
  try {
    await boot({ fromReconnect: true });
  } catch {
    // boot() marks the connection lost again if it still fails.
  }
}

/**
 * Mark the LAN server as unreachable and start exponential backoff retries.
 */
function reportConnectionLost() {
  const wasOnline = connectionOnline;
  connectionOnline = false;
  showConnectionBanner();
  if (wasOnline) {
    connectionBackoffMs = CONNECTION_BACKOFF_START_MS;
  }
  if (connectionProbeTimer) {
    clearTimeout(connectionProbeTimer);
    connectionProbeTimer = null;
  }
  if (!connectionChecking && !connectionRetryTimer) {
    scheduleOfflineRetry();
  }
}

/**
 * @param {{ reloadIfNeeded?: boolean }} [opts]
 */
function reportConnectionOk(opts = {}) {
  const wasOffline = !connectionOnline;
  connectionOnline = true;
  connectionBackoffMs = CONNECTION_BACKOFF_START_MS;
  connectionNextRetryAt = 0;
  clearConnectionTimers();
  hideConnectionBanner();
  scheduleOnlineProbe();
  if (wasOffline && opts.reloadIfNeeded !== false) {
    void onConnectionRestored();
  }
}

/**
 * @returns {Promise<boolean>}
 */
async function probeConnection() {
  if (connectionChecking) return connectionOnline;
  connectionChecking = true;
  const retry = connectionRetryBtn();
  if (retry) retry.disabled = true;
  updateConnectionCountdown();
  try {
    const res = await fetch("/api/health", {
      method: "GET",
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = await res.json().catch(() => ({}));
    if (data && data.ok === false) throw new Error("health not ok");
    // Health probe restores the session (reload catalog only if boot never succeeded).
    reportConnectionOk({ reloadIfNeeded: true });
    return true;
  } catch {
    reportConnectionLost();
    if (!connectionRetryTimer) scheduleOfflineRetry();
    return false;
  } finally {
    connectionChecking = false;
    const btn = connectionRetryBtn();
    if (btn) btn.disabled = false;
    updateConnectionCountdown();
  }
}

function bindConnectionBanner() {
  connectionRetryBtn()?.addEventListener("click", () => {
    if (connectionChecking) return;
    if (connectionRetryTimer) {
      clearTimeout(connectionRetryTimer);
      connectionRetryTimer = null;
    }
    // Manual retry: next wait starts at 10s (not the 1s auto floor).
    connectionBackoffMs = CONNECTION_BACKOFF_MANUAL_MS;
    void probeConnection();
  });
}

function startConnectionMonitor() {
  if (connectionMonitorStarted) return;
  connectionMonitorStarted = true;
  bindConnectionBanner();
  window.addEventListener("offline", () => {
    reportConnectionLost();
  });
  window.addEventListener("online", () => {
    connectionBackoffMs = CONNECTION_BACKOFF_START_MS;
    void probeConnection();
  });
  scheduleOnlineProbe();
}

async function api(path, options) {
  let res;
  try {
    res = await fetch(path, {
      headers: { "Content-Type": "application/json", ...(options?.headers || {}) },
      ...options,
    });
  } catch (err) {
    reportConnectionLost();
    throw err;
  }
  // Reachable again: clear the banner, but let the caller finish (avoid nested boot).
  if (!connectionOnline) {
    reportConnectionOk({ reloadIfNeeded: false });
  }
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(data.error || data.message || `HTTP ${res.status}`);
  }
  return data;
}

function currentWatchMode() {
  return watchMode;
}

function bindBrandHome() {
  document.querySelectorAll("[data-home]").forEach((el) => {
    el.addEventListener("click", (event) => {
      event.preventDefault();
      navigate({ page: "home" });
    });
  });
}

/**
 * @param {boolean} playing
 */
async function pushPresence(playing) {
  /** @type {{ clientId: string, mode: string, playing: boolean, title?: string, path?: string }} */
  const body = {
    clientId: viewerClientId(),
    mode: currentWatchMode(),
    playing,
  };
  if (playing && nav.page === "watch") {
    body.title = nav.title;
    body.path = nav.path;
  }
  const payload = JSON.stringify(body);
  try {
    await api("/api/presence", {
      method: "POST",
      body: payload,
      keepalive: true,
    });
  } catch {
    // Fallback for background tabs / unload: best-effort beacon.
    try {
      if (typeof navigator !== "undefined" && typeof navigator.sendBeacon === "function") {
        const blob = new Blob([payload], { type: "application/json" });
        navigator.sendBeacon("/api/presence", blob);
      }
    } catch {
      // Ignore: Center will simply show no live session.
    }
  }
}

function startPresenceLoop() {
  if (presenceTimer) {
    clearInterval(presenceTimer);
    presenceTimer = null;
  }
  void pushPresence(true);
  presenceTimer = setInterval(() => {
    if (nav.page === "watch" && document.visibilityState !== "hidden") {
      void pushPresence(true);
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

function syncPresenceWithNav() {
  if (nav.page === "watch" && watchMode === "solo") startPresenceLoop();
  else stopPresenceLoop();
}

document.addEventListener("visibilitychange", () => {
  if (
    document.visibilityState === "visible" &&
    nav.page === "watch" &&
    watchMode === "solo"
  ) {
    void pushPresence(true);
  }
});

function displayName(item) {
  return (item.displayTitle && item.displayTitle.trim()) || item.title;
}

function findSeries(id) {
  return catalog?.series.find((s) => s.id === id);
}

/** @param {string} text */
function slugify(text) {
  const slug = String(text || "")
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "item";
}

/**
 * @template {{ id: string, tmdbId?: number|null }} T
 * @param {T[]} items
 * @param {(item: T) => string} getTitle
 */
function buildSlugMaps(items, getTitle) {
  /** @type {Map<string, T>} */
  const bySlug = new Map();
  /** @type {Map<string, string>} */
  const byId = new Map();
  const bases = items.map((item) => ({ item, base: slugify(getTitle(item)) }));
  /** @type {Map<string, number>} */
  const counts = new Map();
  for (const { base } of bases) {
    counts.set(base, (counts.get(base) || 0) + 1);
  }
  for (const { item, base } of bases) {
    let slug = base;
    if ((counts.get(base) || 0) > 1) {
      const suffix = String(item.tmdbId ?? item.id)
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "");
      slug = suffix ? `${base}-${suffix}` : base;
    }
    let candidate = slug;
    let n = 2;
    while (bySlug.has(candidate)) {
      candidate = `${slug}-${n++}`;
    }
    bySlug.set(candidate, item);
    byId.set(item.id, candidate);
  }
  return { bySlug, byId };
}

/** @param {{ series: Series[], movies: Movie[] } | null} cat */
function rebuildSlugs(cat) {
  if (!cat) {
    seriesBySlug = new Map();
    seriesSlugById = new Map();
    movieBySlug = new Map();
    movieSlugById = new Map();
    return;
  }
  const seriesMaps = buildSlugMaps(cat.series, (s) => displayName(s));
  seriesBySlug = seriesMaps.bySlug;
  seriesSlugById = seriesMaps.byId;
  const movieMaps = buildSlugMaps(cat.movies, (m) => displayName(m));
  movieBySlug = movieMaps.bySlug;
  movieSlugById = movieMaps.byId;
}

/** @param {string} seriesId */
function seriesSlug(seriesId) {
  return seriesSlugById.get(seriesId) || slugify(seriesId);
}

/** @param {string} movieId */
function movieSlug(movieId) {
  return movieSlugById.get(movieId) || slugify(movieId);
}

/** @param {number} n */
function formatEpisode(n) {
  return String(n).padStart(2, "0");
}

/**
 * Resolve a series URL segment (slug or legacy catalog id).
 * @param {{ series: Series[] } | null | undefined} cat
 * @param {string} ref
 */
function findSeriesByRef(cat, ref) {
  const decoded = decodeURIComponent(ref);
  return seriesBySlug.get(decoded) || cat?.series.find((s) => s.id === decoded) || null;
}

/**
 * Resolve a movie URL segment (slug or legacy catalog id).
 * @param {{ movies: Movie[] } | null | undefined} cat
 * @param {string} ref
 */
function findMovieByRef(cat, ref) {
  const decoded = decodeURIComponent(ref);
  return movieBySlug.get(decoded) || cat?.movies.find((m) => m.id === decoded) || null;
}

/**
 * @param {Nav} state
 * @returns {string}
 */
function pathForNav(state) {
  if (state.page === "home") return "/";
  if (state.page === "series") {
    return `/series/${encodeURIComponent(seriesSlug(state.seriesId))}`;
  }
  if (state.page === "season") {
    return `/series/${encodeURIComponent(seriesSlug(state.seriesId))}/s${state.season}`;
  }
  if (state.page === "watch" && state.kind === "episode") {
    return `/series/${encodeURIComponent(seriesSlug(state.seriesId))}/s${state.season}/e${formatEpisode(state.episode)}?mode=${watchMode}`;
  }
  if (state.page === "watch" && state.kind === "movie") {
    return `/movies/${encodeURIComponent(movieSlug(state.movieId))}?mode=${watchMode}`;
  }
  return `${window.location.pathname}${window.location.search}`;
}

/**
 * @param {string} mediaPath
 * @param {{ series: Series[], movies: Movie[] } | null} [cat]
 * @returns {NavWatch | null}
 */
function findPlayableByPath(mediaPath, cat = catalog) {
  if (!cat) return null;
  for (const movie of cat.movies) {
    if (movie.path === mediaPath) {
      return {
        page: "watch",
        kind: "movie",
        movieId: movie.id,
        path: movie.path,
        title: displayName(movie),
      };
    }
  }
  for (const show of cat.series) {
    for (const season of show.seasons) {
      for (const ep of season.episodes) {
        if (ep.path === mediaPath) {
          return {
            page: "watch",
            kind: "episode",
            seriesId: show.id,
            season: season.number,
            episode: ep.episode,
            path: ep.path,
            title: ep.title?.trim() || ep.label,
          };
        }
      }
    }
  }
  return null;
}

/**
 * Parse the current location into a nav state using the loaded catalog.
 * @param {{ series: Series[], movies: Movie[] } | null} cat
 * @returns {Nav | null} null when the path does not match any route
 */
function navFromLocation(cat) {
  const rawPath = window.location.pathname.replace(/\/+$/, "") || "/";
  const search = window.location.search;

  if (rawPath === "/") return { page: "home" };

  const movieMatch = rawPath.match(/^\/movies\/([^/]+)$/);
  if (movieMatch) {
    const movie = findMovieByRef(cat, movieMatch[1]);
    if (!movie) {
      return { page: "not-found", message: "Film introuvable dans le catalogue." };
    }
    return {
      page: "watch",
      kind: "movie",
      movieId: movie.id,
      path: movie.path,
      title: displayName(movie),
    };
  }

  // /series/:slug/s1/e01
  const episodeMatch = rawPath.match(/^\/series\/([^/]+)\/s(\d+)\/e(\d+)$/i);
  if (episodeMatch) {
    const show = findSeriesByRef(cat, episodeMatch[1]);
    const seasonNum = Number(episodeMatch[2]);
    const episodeNum = Number(episodeMatch[3]);
    const season = show?.seasons.find((s) => s.number === seasonNum);
    const ep = season?.episodes.find((item) => item.episode === episodeNum);
    if (!show || !season || !ep) {
      return { page: "not-found", message: "Épisode introuvable dans le catalogue." };
    }
    return {
      page: "watch",
      kind: "episode",
      seriesId: show.id,
      season: season.number,
      episode: ep.episode,
      path: ep.path,
      title: ep.title?.trim() || ep.label,
    };
  }

  // /series/:slug/s1  (also accept legacy /series/:id/s/1)
  const seasonMatch =
    rawPath.match(/^\/series\/([^/]+)\/s(\d+)$/i) ||
    rawPath.match(/^\/series\/([^/]+)\/s\/(\d+)$/i);
  if (seasonMatch) {
    const show = findSeriesByRef(cat, seasonMatch[1]);
    const season = Number(seasonMatch[2]);
    if (!show || !show.seasons.some((s) => s.number === season)) {
      return { page: "not-found", message: "Saison introuvable dans le catalogue." };
    }
    return { page: "season", seriesId: show.id, season };
  }

  const seriesMatch = rawPath.match(/^\/series\/([^/]+)$/);
  if (seriesMatch) {
    const show = findSeriesByRef(cat, seriesMatch[1]);
    if (!show) {
      return { page: "not-found", message: "Série introuvable dans le catalogue." };
    }
    return { page: "series", seriesId: show.id };
  }

  // Legacy deep-link: /watch?path=… → resolve then canonicalize via pathForNav
  if (rawPath === "/watch") {
    const mediaPath = new URLSearchParams(search).get("path");
    if (!mediaPath) {
      return { page: "not-found", message: "Lien de lecture incomplet." };
    }
    const playable = findPlayableByPath(mediaPath, cat);
    if (!playable) {
      return {
        page: "not-found",
        message: "Média introuvable dans le catalogue.",
      };
    }
    return playable;
  }

  return null;
}

/**
 * @param {Nav} next
 * @param {{ replace?: boolean, fullscreen?: boolean }} [opts]
 */
function navigate(next, opts = {}) {
  const prev = nav;
  nav = next;
  if (opts.fullscreen && next.page === "watch") {
    pendingFullscreen = true;
  }
  const url = pathForNav(nav);
  if (opts.replace) {
    history.replaceState({}, "", url);
  } else {
    history.pushState({}, "", url);
  }
  if (prev.page === "watch" && next.page !== "watch") {
    destroyJamHost();
    void exitFullscreen();
  }
  render();
  syncPresenceWithNav();
}

function applyLocationFromHistory() {
  destroyJamHost();
  applyWatchModeFromLocation();
  const resolved = navFromLocation(catalog);
  if (!resolved) {
    nav = { page: "home" };
    history.replaceState({}, "", "/");
  } else {
    nav = resolved;
    const canonical = pathForNav(nav);
    const current = `${window.location.pathname}${window.location.search}`;
    if (nav.page !== "not-found" && canonical !== current) {
      history.replaceState({}, "", canonical);
    }
  }
  pendingFullscreen = false;
  void exitFullscreen();
  render();
  syncPresenceWithNav();
}

function posterHtml(url, title) {
  if (url) {
    return `<span class="poster-frame"><img src="${escapeHtml(url)}" alt="" loading="lazy" /></span>`;
  }
  const letter = (title || "?").trim().slice(0, 1).toUpperCase() || "?";
  return `<span class="poster-frame"><span class="poster-fallback">${escapeHtml(letter)}</span></span>`;
}

function chevron() {
  return `<span class="chevron" aria-hidden="true">›</span>`;
}

function randomButton(attrs) {
  return `<button type="button" class="btn-random" ${attrs} title="Lecture aléatoire">Lecture aléatoire</button>`;
}

function render() {
  if (!catalog) {
    main.innerHTML = `<p class="muted">Chargement de la bibliothèque…</p>`;
    return;
  }
  if (nav.page === "not-found") {
    main.innerHTML = `
      <nav class="breadcrumb">
        <button type="button" data-nav="home">Bibliothèque</button>
        ${chevron()}
        <span class="current">Introuvable</span>
      </nav>
      <p class="muted">${escapeHtml(nav.message)}</p>
      <p><button type="button" class="btn-random" data-nav="home">Retour à la bibliothèque</button></p>
    `;
    bindNav();
    return;
  }
  if (nav.page === "home") {
    main.innerHTML = renderHome();
    bindHome();
    bindRandom();
    return;
  }
  if (nav.page === "series") {
    main.innerHTML = renderSeries(nav.seriesId);
    bindNav();
    bindSeries();
    bindRandom();
    return;
  }
  if (nav.page === "season") {
    main.innerHTML = renderSeason(nav.seriesId, nav.season);
    bindNav();
    bindSeason();
    bindRandom();
    return;
  }
  if (nav.page === "watch") {
    destroyJamHost();
    if (watchMode === "jam") {
      main.innerHTML = renderJamWatch();
      bindNav();
      void bootJamWatch();
    } else {
      main.innerHTML = renderWatch();
      bindNav();
      void bootPlayer();
    }
    return;
  }
}

function renderHome() {
  const series = catalog?.series ?? [];
  const movies = catalog?.movies ?? [];
  if (series.length === 0 && movies.length === 0) {
    return `
      <nav class="breadcrumb"><span class="current">Bibliothèque</span></nav>
      <p class="muted">Aucun média. Préparez le catalogue dans Neolingua Center.</p>
    `;
  }
  return `
    <nav class="breadcrumb"><span class="current">Bibliothèque</span></nav>
    ${
      series.length
        ? `<section>
            <div class="section-head">
              <h2>Séries</h2>
              <span class="section-count">${series.length}</span>
              ${randomButton('data-random="all-series"')}
            </div>
            <div class="poster-grid">
              ${series
                .map(
                  (show) => `
                <button type="button" class="poster-card" data-series="${escapeHtml(show.id)}">
                  ${posterHtml(show.posterUrl, displayName(show))}
                  <span class="poster-title">${escapeHtml(displayName(show))}</span>
                  <span class="poster-sub">${show.seasons.length} saison${show.seasons.length > 1 ? "s" : ""}</span>
                </button>`,
                )
                .join("")}
            </div>
          </section>`
        : ""
    }
    ${
      movies.length
        ? `<section>
            <div class="section-head">
              <h2>Films</h2>
              <span class="section-count">${movies.length}</span>
              ${randomButton('data-random="movies"')}
            </div>
            <div class="poster-grid">
              ${movies
                .map(
                  (movie) => `
                <button type="button" class="poster-card" data-movie="${escapeHtml(movie.id)}">
                  ${posterHtml(movie.posterUrl, displayName(movie))}
                  <span class="poster-title">${escapeHtml(displayName(movie))}</span>
                  <span class="poster-sub">${movie.year ? escapeHtml(String(movie.year)) : "Film"}</span>
                </button>`,
                )
                .join("")}
            </div>
          </section>`
        : ""
    }
  `;
}

function renderSeries(seriesId) {
  const show = findSeries(seriesId);
  if (!show) return `<p class="muted">Série introuvable.</p>`;
  return `
    <nav class="breadcrumb">
      <button type="button" data-nav="home">Bibliothèque</button>
      ${chevron()}
      <span class="current">${escapeHtml(displayName(show))}</span>
    </nav>
    <div class="section-head">
      <h2>Saisons</h2>
      <span class="section-count">${show.seasons.length}</span>
      ${randomButton(`data-random="series" data-series-id="${escapeHtml(show.id)}"`)}
    </div>
    <div class="poster-grid">
      ${show.seasons
        .map(
          (season) => `
        <button type="button" class="poster-card" data-season="${season.number}">
          ${posterHtml(season.posterUrl, season.title || `Saison ${season.number}`)}
          <span class="poster-title">${escapeHtml(season.title?.trim() || `Saison ${season.number}`)}</span>
          <span class="poster-sub">${season.episodes.length} épisode${season.episodes.length > 1 ? "s" : ""}</span>
        </button>`,
        )
        .join("")}
    </div>
  `;
}

function renderSeason(seriesId, seasonNumber) {
  const show = findSeries(seriesId);
  const season = show?.seasons.find((s) => s.number === seasonNumber);
  if (!show || !season) return `<p class="muted">Saison introuvable.</p>`;
  return `
    <nav class="breadcrumb">
      <button type="button" data-nav="home">Bibliothèque</button>
      ${chevron()}
      <button type="button" data-nav="series">${escapeHtml(displayName(show))}</button>
      ${chevron()}
      <span class="current">${escapeHtml(season.title?.trim() || `Saison ${season.number}`)}</span>
    </nav>
    <div class="section-head">
      <h2>Épisodes</h2>
      <span class="section-count">${season.episodes.length}</span>
      ${randomButton(
        `data-random="season" data-series-id="${escapeHtml(show.id)}" data-season="${season.number}"`,
      )}
    </div>
    <ul class="episode-list">
      ${season.episodes
        .map((ep) => {
          const title = ep.title?.trim() || ep.label;
          const attrs = `data-path="${escapeHtml(ep.path)}" data-title="${escapeHtml(title)}" data-episode="${ep.episode}"`;
          const actions = episodeSeasonActions(ep, attrs);
          return `
            <li class="episode-row">
              <span class="episode-num">${String(ep.episode).padStart(2, "0")}</span>
              <span class="episode-copy">
                <span class="episode-title">${escapeHtml(title)}</span>
                <span class="episode-label">${escapeHtml(ep.label)}</span>
              </span>
              ${actions}
            </li>`;
        })
        .join("")}
    </ul>
  `;
}

/**
 * @param {Episode} ep
 * @param {string} attrs
 */
function episodeSeasonActions(ep, attrs) {
  const status = ep.prep?.status ?? "missing";
  if (status === "ready" || status === "partial") {
    return `<span class="episode-actions">
      <button type="button" class="badge badge-jam" ${attrs} data-watch-mode="jam">Jam</button>
      <button type="button" class="badge badge-ready" ${attrs} data-watch-mode="solo">Lire</button>
    </span>`;
  }
  if (status === "processing" || status === "queued") {
    const pct = Math.max(0, Math.min(100, Math.round(ep.prep?.progress ?? 0)));
    const label = prepBusyLabel(status, pct);
    return `<span class="episode-actions">
      <button
        type="button"
        class="badge badge-prep"
        data-prepare-path="${escapeHtml(ep.path)}"
        aria-label="Préparation ${escapeHtml(label)}"
        disabled
      >
        <span class="badge-prep-fill" style="width: ${pct}%"></span>
        <span class="badge-prep-label">${escapeHtml(label)}</span>
      </button>
    </span>`;
  }
  const actionLabel = status === "error" ? "Réessayer" : "Préparer";
  const extra = status === "error" ? " badge-action--error" : "";
  const tip = ep.prep?.message?.trim()
    ? ` title="${escapeHtml(ep.prep.message.trim())}"`
    : "";
  return `<span class="episode-actions">
    <button type="button" class="badge badge-action${extra}" data-prepare-path="${escapeHtml(ep.path)}"${tip}>${actionLabel}</button>
  </span>`;
}

function renderWatch() {
  return `
    <nav class="breadcrumb">
      <button type="button" data-nav="home">Bibliothèque</button>
      ${
        nav.page === "watch" && nav.kind === "episode" && nav.seriesId
          ? `${chevron()}
             <button type="button" data-nav="series">${escapeHtml(displayName(findSeries(nav.seriesId) || { title: "Série" }))}</button>
             ${chevron()}
             <button type="button" data-nav="season">Saison ${nav.season}</button>
             ${chevron()}`
          : `${chevron()}`
      }
      <span class="current">${escapeHtml(nav.page === "watch" ? nav.title : "")}</span>
    </nav>
    <div class="player-shell" id="player-shell">
      <div class="player-frame" id="player-frame">
        <video id="player" playsinline></video>
        <div class="prep-overlay" id="prep-overlay" role="status" aria-live="polite">
          <p class="prep-overlay-title">Chargement</p>
          <p class="prep-overlay-msg" id="prep-overlay-msg">Un instant…</p>
          <div
            class="prep-progress"
            id="prep-progress"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow="0"
            aria-label="Progression de la préparation"
          >
            <div class="prep-progress-track">
              <div class="prep-progress-fill" id="prep-progress-fill"></div>
            </div>
            <span class="prep-progress-pct" id="prep-progress-pct">0 %</span>
          </div>
        </div>
        <div class="subs" id="subs">
          <p class="sub-line is-empty" id="sub-en"></p>
          <p class="sub-line is-empty" id="sub-fr" hidden></p>
        </div>
        <button type="button" class="btn-skip-intro" id="skip-intro" hidden>
          Passer le générique
        </button>
      </div>
      <div class="player-chrome" id="player-chrome">
        <div class="transport" id="transport">
          <input
            class="scrubber"
            id="scrubber"
            type="range"
            min="0"
            max="0"
            step="0.1"
            value="0"
            aria-label="Position"
          />
          <div class="transport-row">
            <p class="timecode" id="timecode">0:00 / 0:00</p>
            <div class="transport-actions">
              <button class="btn-transport" id="to-start" type="button" aria-label="Revenir au début" title="Début">
                <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M6 5h2v14H6zm3.5 7l9.5 6.5V5.5z"/>
                </svg>
              </button>
              <button class="btn-transport" id="back-5" type="button" aria-label="Reculer de 5 secondes" title="−5 s">
                <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M12 5V2L7 6.5 12 11V8a5 5 0 1 1-4.9 6H5.08A7 7 0 1 0 12 5z"/>
                  <text x="12" y="15.2" text-anchor="middle" class="icon-num">5</text>
                </svg>
              </button>
              <button class="btn-transport btn-play" id="play-pause" type="button" aria-label="Lecture" title="Lecture">
                <svg class="icon icon-play" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M8 5v14l11-7z"/>
                </svg>
                <svg class="icon icon-pause" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M7 5h3v14H7zm7 0h3v14h-3z"/>
                </svg>
              </button>
              <button class="btn-transport" id="fwd-5" type="button" aria-label="Avancer de 5 secondes" title="+5 s">
                <svg class="icon" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M12 5V2l5 4.5L12 11V8a5 5 0 1 0 4.9 6h2.02A7 7 0 1 1 12 5z"/>
                  <text x="12" y="15.2" text-anchor="middle" class="icon-num">5</text>
                </svg>
              </button>
            </div>
            <button type="button" id="toggle-fullscreen" class="btn-fullscreen" title="Plein écran">
              Plein écran
            </button>
          </div>
        </div>
        <div class="player-toolbar" aria-label="Sous-titres">
          <div class="sub-lang-control">
            <button type="button" class="btn-sub-size" id="size-en-down" aria-label="Réduire les sous-titres anglais" title="Réduire EN">−</button>
            <button type="button" id="toggle-en" class="${showEn ? "is-active" : ""}" aria-pressed="${showEn ? "true" : "false"}">EN</button>
            <button type="button" class="btn-sub-size" id="size-en-up" aria-label="Agrandir les sous-titres anglais" title="Agrandir EN">+</button>
          </div>
          <div class="sub-lang-control">
            <button type="button" class="btn-sub-size" id="size-fr-down" aria-label="Réduire les sous-titres français" title="Réduire FR">−</button>
            <button type="button" id="toggle-fr" class="${showFr ? "is-active" : ""}" aria-pressed="${showFr ? "true" : "false"}">FR</button>
            <button type="button" class="btn-sub-size" id="size-fr-up" aria-label="Agrandir les sous-titres français" title="Agrandir FR">+</button>
          </div>
        </div>
      </div>
    </div>
  `;
}

/**
 * @param {NavWatch} next
 * @param {"solo" | "jam"} [mode]
 */
function openWatch(next, mode = "solo") {
  setWatchMode(mode);
  // Solo: fullscreen on open. Jam: fullscreen on "Créer le salon" (QR screen).
  navigate(next, mode === "solo" ? { fullscreen: true } : {});
}

function jamWatchMeta() {
  if (nav.page !== "watch") {
    return { path: "", title: "", seriesTitle: "", seasonLabel: "" };
  }
  if (nav.kind === "episode") {
    const show = findSeries(nav.seriesId);
    const epNum = String(nav.episode).padStart(2, "0");
    return {
      path: nav.path,
      title: nav.title,
      seriesTitle: show ? displayName(show) : "",
      seasonLabel: `Saison ${nav.season} · Épisode ${epNum}`,
    };
  }
  const movie = catalog?.movies.find((m) => m.id === nav.movieId);
  return {
    path: nav.path,
    title: nav.title,
    seriesTitle: "Film",
    seasonLabel: movie?.year ? String(movie.year) : "",
  };
}

function renderJamWatch() {
  return `
    <nav class="breadcrumb">
      <button type="button" data-nav="home">Bibliothèque</button>
      ${
        nav.page === "watch" && nav.kind === "episode" && nav.seriesId
          ? `${chevron()}
             <button type="button" data-nav="series">${escapeHtml(displayName(findSeries(nav.seriesId) || { title: "Série" }))}</button>
             ${chevron()}
             <button type="button" data-nav="season">Saison ${nav.season}</button>
             ${chevron()}`
          : `${chevron()}`
      }
      <span class="current">${escapeHtml(nav.page === "watch" ? nav.title : "")}</span>
    </nav>
    <div id="jam-host" class="jam jam-display"></div>
  `;
}

async function bootJamWatch() {
  if (nav.page !== "watch" || watchMode !== "jam") return;
  const root = document.getElementById("jam-host");
  if (!root) return;
  const meta = jamWatchMeta();
  destroyJamHost();
  try {
    jamHost = await mountJamHost({
      root,
      path: meta.path,
      title: meta.title,
      seriesTitle: meta.seriesTitle,
      seasonLabel: meta.seasonLabel,
      onCancel: () => navigate({ page: "home" }),
    });
  } catch (err) {
    root.innerHTML = `<p class="muted">${escapeHtml(err instanceof Error ? err.message : String(err))}</p>`;
  }
}

function bindHome() {
  document.querySelectorAll("[data-series]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const seriesId = btn.getAttribute("data-series");
      if (!seriesId) return;
      navigate({ page: "series", seriesId });
    });
  });
  document.querySelectorAll("[data-movie]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.getAttribute("data-movie");
      const movie = catalog?.movies.find((m) => m.id === id);
      if (!movie) return;
      openWatch({
        page: "watch",
        kind: "movie",
        movieId: movie.id,
        path: movie.path,
        title: displayName(movie),
      });
    });
  });
}

function bindNav() {
  document.querySelectorAll("[data-nav]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const target = btn.getAttribute("data-nav");
      if (target === "home") {
        navigate({ page: "home" });
        return;
      }
      if (target === "series" && nav.page !== "home") {
        const seriesId = "seriesId" in nav ? nav.seriesId : null;
        if (seriesId) navigate({ page: "series", seriesId });
        return;
      }
      if (target === "season" && nav.page === "watch" && nav.kind === "episode") {
        navigate({ page: "season", seriesId: nav.seriesId, season: nav.season });
      }
    });
  });
}

function bindSeries() {
  document.querySelectorAll("[data-season]").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (nav.page !== "series") return;
      navigate({
        page: "season",
        seriesId: nav.seriesId,
        season: Number(btn.getAttribute("data-season")),
      });
    });
  });
}

/** Paths currently being prepared from the season list (avoid double-start). */
const preparingPaths = new Set();

function bindSeason() {
  document.querySelectorAll("[data-watch-mode][data-path]").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (nav.page !== "season") return;
      const path = btn.getAttribute("data-path");
      const episodeAttr = btn.getAttribute("data-episode");
      if (!path || episodeAttr == null) return;
      const mode = btn.getAttribute("data-watch-mode") === "jam" ? "jam" : "solo";
      openWatch(
        {
          page: "watch",
          kind: "episode",
          seriesId: nav.seriesId,
          season: nav.season,
          episode: Number(episodeAttr),
          path,
          title: btn.getAttribute("data-title") || "Épisode",
        },
        mode,
      );
    });
  });
  document.querySelectorAll("[data-prepare-path]").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (btn.disabled) return;
      const path = btn.getAttribute("data-prepare-path");
      if (!path) return;
      void prepareEpisodeInPlace(path);
    });
  });
}

/**
 * @param {string} path
 * @param {Prep} prep
 */
function setEpisodePrep(path, prep) {
  if (!catalog) return;
  for (const show of catalog.series) {
    for (const season of show.seasons) {
      for (const ep of season.episodes) {
        if (ep.path === path) {
          ep.prep = prep;
          return;
        }
      }
    }
  }
}

/**
 * @param {any} status
 * @returns {Prep}
 */
function prepFromApiStatus(status) {
  const video = status?.video;
  const vs = video?.status;
  if (isPlaybackReady(status)) {
    return {
      status: "ready",
      video: true,
      progress: 100,
      message: null,
    };
  }
  if (vs === "error") {
    return {
      status: "error",
      video: false,
      progress: null,
      message: (video?.message && String(video.message).trim()) || "Échec de préparation",
    };
  }
  if (vs === "processing" || vs === "queued") {
    return {
      status: vs,
      video: false,
      progress: typeof video?.progress === "number" ? video.progress : 0,
      message: video?.message || null,
    };
  }
  return {
    status: "missing",
    video: false,
    progress: null,
    message: null,
  };
}

/**
 * @param {string} path
 * @param {Prep} prep
 */
/** Queue shows as processing@0 from the API; keep "En file" until real progress. */
function prepBusyLabel(status, pct) {
  if (status === "queued" || pct <= 0) return "En file";
  return `${pct} %`;
}

function paintPrepareButton(path, prep) {
  const btn = [...document.querySelectorAll("[data-prepare-path]")].find(
    (el) => el.getAttribute("data-prepare-path") === path,
  );
  if (!btn) return;
  const status = prep.status ?? "missing";
  if (status === "processing" || status === "queued") {
    const pct = Math.max(0, Math.min(100, Math.round(prep.progress ?? 0)));
    const label = prepBusyLabel(status, pct);
    btn.className = "badge badge-prep";
    btn.disabled = true;
    btn.setAttribute("aria-label", `Préparation ${label}`);
    btn.removeAttribute("title");
    btn.innerHTML = `
      <span class="badge-prep-fill" style="width: ${pct}%"></span>
      <span class="badge-prep-label">${escapeHtml(label)}</span>
    `;
    return;
  }
  if (status === "error") {
    btn.className = "badge badge-action badge-action--error";
    btn.disabled = false;
    btn.textContent = "Réessayer";
    if (prep.message) btn.title = prep.message;
    return;
  }
  btn.className = "badge badge-action";
  btn.disabled = false;
  btn.textContent = "Préparer";
}

/**
 * Prepare an episode on the season page without opening the player.
 * @param {string} path
 */
async function prepareEpisodeInPlace(path) {
  if (preparingPaths.has(path)) return;
  preparingPaths.add(path);
  setEpisodePrep(path, {
    status: "processing",
    video: false,
    progress: 0,
    message: "Démarrage…",
  });
  paintPrepareButton(path, {
    status: "processing",
    progress: 0,
    message: "Démarrage…",
  });
  try {
    await api("/api/prepare-video", {
      method: "POST",
      body: JSON.stringify({ path }),
    });
    for (;;) {
      const status = await api(`/api/status?path=${encodeURIComponent(path)}`);
      const prep = prepFromApiStatus(status);
      setEpisodePrep(path, prep);
      if (nav.page === "season") {
        if (prep.status === "ready") {
          render();
          return;
        }
        paintPrepareButton(path, prep);
      } else if (prep.status === "ready" || prep.status === "error") {
        return;
      }
      if (prep.status === "error") return;
      await sleep(700);
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    const prep = {
      status: "error",
      video: false,
      progress: null,
      message,
    };
    setEpisodePrep(path, prep);
    if (nav.page === "season") paintPrepareButton(path, prep);
  } finally {
    preparingPaths.delete(path);
  }
}

/**
 * @typedef {{ kind: 'episode', seriesId: string, season: number, episode: number, path: string, title: string } | { kind: 'movie', movieId: string, path: string, title: string }} Playable
 */

/** @returns {Playable[]} */
function episodesFromShow(show) {
  /** @type {Playable[]} */
  const items = [];
  for (const season of show.seasons) {
    for (const ep of season.episodes) {
      items.push({
        kind: "episode",
        seriesId: show.id,
        season: season.number,
        episode: ep.episode,
        path: ep.path,
        title: ep.title?.trim() || ep.label,
      });
    }
  }
  return items;
}

/** @returns {Playable[]} */
function playablePool(scope, seriesId, seasonNumber) {
  if (!catalog) return [];
  if (scope === "movies") {
    return catalog.movies.map((movie) => ({
      kind: "movie",
      movieId: movie.id,
      path: movie.path,
      title: displayName(movie),
    }));
  }
  if (scope === "all-series") {
    return catalog.series.flatMap((show) => episodesFromShow(show));
  }
  if (scope === "series" && seriesId) {
    const show = findSeries(seriesId);
    return show ? episodesFromShow(show) : [];
  }
  if (scope === "season" && seriesId && seasonNumber != null) {
    const show = findSeries(seriesId);
    const season = show?.seasons.find((s) => s.number === seasonNumber);
    if (!show || !season) return [];
    return season.episodes.map((ep) => ({
      kind: "episode",
      seriesId: show.id,
      season: season.number,
      episode: ep.episode,
      path: ep.path,
      title: ep.title?.trim() || ep.label,
    }));
  }
  return [];
}

function bindRandom() {
  document.querySelectorAll("[data-random]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const scope = btn.getAttribute("data-random");
      const seriesId = btn.getAttribute("data-series-id");
      const seasonAttr = btn.getAttribute("data-season");
      const seasonNumber = seasonAttr != null ? Number(seasonAttr) : null;
      const pool = playablePool(scope, seriesId, seasonNumber);
      if (pool.length === 0) return;
      const pick = pool[Math.floor(Math.random() * pool.length)];
      if (pick.kind === "movie") {
        openWatch({
          page: "watch",
          kind: "movie",
          movieId: pick.movieId,
          path: pick.path,
          title: pick.title,
        });
      } else {
        openWatch({
          page: "watch",
          kind: "episode",
          seriesId: pick.seriesId,
          season: pick.season,
          episode: pick.episode,
          path: pick.path,
          title: pick.title,
        });
      }
    });
  });
}

/**
 * @param {string} message
 * @param {number | null} progress
 * @param {"busy" | "error"} [tone]
 */
function showPrepOverlay(message, progress, tone = "busy") {
  const overlay = document.getElementById("prep-overlay");
  const title = overlay?.querySelector(".prep-overlay-title");
  const msg = document.getElementById("prep-overlay-msg");
  const bar = document.getElementById("prep-progress");
  const fill = document.getElementById("prep-progress-fill");
  const pctEl = document.getElementById("prep-progress-pct");
  if (!overlay || !msg || !bar || !fill || !pctEl) return;

  overlay.hidden = false;
  overlay.classList.toggle("is-error", tone === "error");
  if (title) {
    title.textContent = tone === "error" ? "Préparation impossible" : "Préparation";
  }
  msg.textContent = message || (tone === "error" ? "La préparation a échoué." : "Préparation…");

  if (progress == null || tone === "error") {
    bar.classList.add("is-indeterminate");
    bar.removeAttribute("aria-valuenow");
    fill.style.width = tone === "error" ? "100%" : "35%";
    pctEl.textContent = "";
    return;
  }

  const pct = Math.max(0, Math.min(100, Math.round(progress)));
  bar.classList.remove("is-indeterminate");
  bar.setAttribute("aria-valuenow", String(pct));
  fill.style.width = `${pct}%`;
  pctEl.textContent = `${pct} %`;
}

function hidePrepOverlay() {
  const overlay = document.getElementById("prep-overlay");
  if (!overlay) return;
  overlay.hidden = true;
  overlay.classList.remove("is-error");
}

function applySubtitleSizes() {
  document.documentElement.style.setProperty(
    "--sub-en-size",
    `${SIZE_STEPS[sizeEnIndex]}rem`,
  );
  document.documentElement.style.setProperty(
    "--sub-fr-size",
    `${SIZE_STEPS[sizeFrIndex]}rem`,
  );
  const enDown = document.getElementById("size-en-down");
  const enUp = document.getElementById("size-en-up");
  const frDown = document.getElementById("size-fr-down");
  const frUp = document.getElementById("size-fr-up");
  if (enDown) enDown.disabled = sizeEnIndex <= 0;
  if (enUp) enUp.disabled = sizeEnIndex >= SIZE_STEPS.length - 1;
  if (frDown) frDown.disabled = sizeFrIndex <= 0;
  if (frUp) frUp.disabled = sizeFrIndex >= SIZE_STEPS.length - 1;
}

/**
 * @param {'en'|'fr'} lang
 * @param {-1|1} delta
 */
function nudgeSubtitleSize(lang, delta) {
  if (lang === "en") {
    sizeEnIndex = clampSizeIndex(sizeEnIndex + delta);
    localStorage.setItem(SUB_EN_SIZE_KEY, String(sizeEnIndex));
  } else {
    sizeFrIndex = clampSizeIndex(sizeFrIndex + delta);
    localStorage.setItem(SUB_FR_SIZE_KEY, String(sizeFrIndex));
  }
  applySubtitleSizes();
}

async function bootPlayer() {
  if (nav.page !== "watch") return;
  const path = nav.path;
  const player = document.getElementById("player");
  const shell = document.getElementById("player-shell");
  const toggleEn = document.getElementById("toggle-en");
  const toggleFr = document.getElementById("toggle-fr");
  const sizeEnDown = document.getElementById("size-en-down");
  const sizeEnUp = document.getElementById("size-en-up");
  const sizeFrDown = document.getElementById("size-fr-down");
  const sizeFrUp = document.getElementById("size-fr-up");
  const toggleFs = document.getElementById("toggle-fullscreen");
  const playPauseBtn = document.getElementById("play-pause");
  const toStartBtn = document.getElementById("to-start");
  const back5Btn = document.getElementById("back-5");
  const fwd5Btn = document.getElementById("fwd-5");
  const scrubber = document.getElementById("scrubber");
  const subEn = document.getElementById("sub-en");
  const subFr = document.getElementById("sub-fr");
  const skipIntroBtn = document.getElementById("skip-intro");
  if (
    !player ||
    !shell ||
    !toggleEn ||
    !toggleFr ||
    !sizeEnDown ||
    !sizeEnUp ||
    !sizeFrDown ||
    !sizeFrUp ||
    !playPauseBtn ||
    !toStartBtn ||
    !back5Btn ||
    !fwd5Btn ||
    !scrubber ||
    !subEn ||
    !subFr ||
    !skipIntroBtn
  ) {
    return;
  }

  introStartSeconds = null;
  introEndSeconds = null;
  introAutoSkipped = false;
  scrubbing = false;
  skipIntroBtn.hidden = true;
  applySubtitleSizes();

  toggleEn.addEventListener("click", () => {
    showEn = !showEn;
    toggleEn.classList.toggle("is-active", showEn);
    toggleEn.setAttribute("aria-pressed", showEn ? "true" : "false");
    updateCueDisplay(player.currentTime);
  });
  toggleFr.addEventListener("click", () => {
    showFr = !showFr;
    toggleFr.classList.toggle("is-active", showFr);
    toggleFr.setAttribute("aria-pressed", showFr ? "true" : "false");
    subFr.hidden = !showFr;
    updateCueDisplay(player.currentTime);
  });
  sizeEnDown.addEventListener("click", () => nudgeSubtitleSize("en", -1));
  sizeEnUp.addEventListener("click", () => nudgeSubtitleSize("en", 1));
  sizeFrDown.addEventListener("click", () => nudgeSubtitleSize("fr", -1));
  sizeFrUp.addEventListener("click", () => nudgeSubtitleSize("fr", 1));
  toggleFs?.addEventListener("click", () => {
    void togglePlayerFullscreen(shell);
  });
  playPauseBtn.addEventListener("click", () => {
    void togglePlayPause(player);
  });
  toStartBtn.addEventListener("click", () => {
    seekToStart(player);
  });
  back5Btn.addEventListener("click", () => {
    seekBy(player, -5);
  });
  fwd5Btn.addEventListener("click", () => {
    seekBy(player, 5);
  });
  skipIntroBtn.addEventListener("click", () => {
    skipIntro(player);
  });
  scrubber.addEventListener("pointerdown", () => {
    scrubbing = true;
    clearChromeHideTimer();
    shell.classList.remove("is-chrome-hidden");
  });
  scrubber.addEventListener("pointerup", () => {
    scrubbing = false;
    player.currentTime = Number(scrubber.value);
    updateCueDisplay(player.currentTime);
    syncTransport(player);
    scheduleChromeHide(shell, player);
  });
  scrubber.addEventListener("input", () => {
    const time = Number(scrubber.value);
    updateCueDisplay(time);
    updateSkipIntroVisibility(time, skipIntroBtn);
    const timecode = document.getElementById("timecode");
    if (timecode) {
      timecode.textContent = `${formatTime(time)} / ${formatTime(player.duration || 0)}`;
    }
  });
  player.addEventListener("timeupdate", () => {
    if (!scrubbing) syncTransport(player);
    const time = player.currentTime;
    updateCueDisplay(time);
    maybeAutoSkipIntro(player, time);
    updateSkipIntroVisibility(time, skipIntroBtn);
  });
  player.addEventListener("loadedmetadata", () => {
    scrubber.max = String(player.duration || 0);
    syncTransport(player);
  });
  player.addEventListener("play", () => {
    updatePlayPauseIcon(player);
    scheduleChromeHide(shell, player);
  });
  player.addEventListener("pause", () => {
    updatePlayPauseIcon(player);
    revealChrome(shell, player, true);
  });
  player.addEventListener("ended", () => {
    updatePlayPauseIcon(player);
    revealChrome(shell, player, true);
    if (purgeCacheAfterWatch) {
      void api("/api/purge-cache", {
        method: "POST",
        body: JSON.stringify({ path }),
      }).catch(() => undefined);
    }
  });
  player.addEventListener("click", () => {
    void togglePlayPause(player);
  });

  shell.addEventListener("mousemove", () => {
    revealChrome(shell, player);
  });
  shell.addEventListener("pointerdown", () => {
    revealChrome(shell, player);
  });
  const chrome = document.getElementById("player-chrome");
  chrome?.addEventListener("mouseenter", () => {
    clearChromeHideTimer();
    shell.classList.remove("is-chrome-hidden");
  });
  chrome?.addEventListener("mouseleave", () => {
    scheduleChromeHide(shell, player);
  });

  document.addEventListener("fullscreenchange", () => {
    syncFullscreenButton();
    revealChrome(shell, player, !isPlayerFullscreen(shell));
  });
  document.addEventListener("webkitfullscreenchange", () => {
    syncFullscreenButton();
    revealChrome(shell, player, !isPlayerFullscreen(shell));
  });
  syncFullscreenButton();
  revealChrome(shell, player);

  if (pendingFullscreen) {
    pendingFullscreen = false;
    void requestFullscreen(shell);
  }

  try {
    const settings = await api("/api/settings").catch(() => ({
      skipIntro: autoSkipIntro,
      purgeCacheAfterWatch,
    }));
    autoSkipIntro = Boolean(settings.skipIntro);
    purgeCacheAfterWatch = Boolean(settings.purgeCacheAfterWatch);

    await ensureReady(path);
    showPrepOverlay("Lancement…", 100);
    player.src = `/api/video?path=${encodeURIComponent(path)}`;
    await loadSubtitles(path);
    applyIntroFromCues();
    hidePrepOverlay();

    const onReady = () => {
      player.removeEventListener("loadedmetadata", onReady);
      maybeAutoSkipIntro(player, player.currentTime);
      updateSkipIntroVisibility(player.currentTime, skipIntroBtn);
      syncTransport(player);
    };
    player.addEventListener("loadedmetadata", onReady);

    await player.play().catch(() => undefined);
    updatePlayPauseIcon(player);
    void requestFullscreen(shell);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    showPrepOverlay(message, null, "error");
  }
}

function isPlayerFullscreen(shell) {
  return Boolean(
    shell &&
      (document.fullscreenElement === shell || document.webkitFullscreenElement === shell),
  );
}

function clearChromeHideTimer() {
  chrome.clear();
}

/** @param {HTMLElement | null | undefined} shell @param {HTMLVideoElement | null | undefined} player @param {boolean} [keepVisible] */
function revealChrome(shell, player, keepVisible = false) {
  chromeShell = shell ?? null;
  chromePlayer = player ?? null;
  chrome.reveal(keepVisible);
}

/** @param {HTMLElement | null | undefined} shell @param {HTMLVideoElement | null | undefined} player */
function scheduleChromeHide(shell, player) {
  chromeShell = shell ?? null;
  chromePlayer = player ?? null;
  chrome.schedule();
}

async function togglePlayPause(player) {
  if (!player.src) return;
  if (player.paused || player.ended) {
    try {
      await player.play();
    } catch {
      showPrepOverlay("Impossible de lancer la lecture", null, "error");
    }
  } else {
    player.pause();
  }
  updatePlayPauseIcon(player);
}

function seekBy(player, deltaSeconds) {
  if (!Number.isFinite(player.duration)) {
    player.currentTime = Math.max(0, player.currentTime + deltaSeconds);
  } else {
    player.currentTime = Math.min(
      player.duration,
      Math.max(0, player.currentTime + deltaSeconds),
    );
  }
  syncTransport(player);
  updateCueDisplay(player.currentTime);
}

function seekToStart(player) {
  player.currentTime = 0;
  syncTransport(player);
  updateCueDisplay(0);
}

function syncTransport(player) {
  const scrubber = document.getElementById("scrubber");
  const timecode = document.getElementById("timecode");
  const current = player.currentTime || 0;
  const duration = player.duration || 0;
  if (scrubber && !scrubbing) {
    scrubber.max = String(duration || 0);
    scrubber.value = String(current);
  }
  if (timecode) {
    timecode.textContent = `${formatTime(current)} / ${formatTime(duration)}`;
  }
  updatePlayPauseIcon(player);
  updateSkipIntroVisibility(current, document.getElementById("skip-intro"));
}

function updatePlayPauseIcon(player) {
  const btn = document.getElementById("play-pause");
  if (!btn) return;
  const playing = Boolean(player.src) && !player.paused && !player.ended;
  btn.classList.toggle("is-playing", playing);
  btn.setAttribute("aria-label", playing ? "Pause" : "Lecture");
  btn.title = playing ? "Pause" : "Lecture";
}

function skipIntro(player) {
  if (introEndSeconds == null) return;
  player.currentTime = introEndSeconds;
  syncTransport(player);
  updateCueDisplay(player.currentTime);
  updateSkipIntroVisibility(player.currentTime, document.getElementById("skip-intro"));
}

/**
 * Auto-skip once when playback first enters the intro window (keeps cold opens).
 * After that, seeking back into the intro is allowed (manual skip still works).
 * @param {HTMLVideoElement} player
 * @param {number} time
 */
function maybeAutoSkipIntro(player, time) {
  if (!autoSkipIntro || introAutoSkipped) return;
  if (introStartSeconds == null || introEndSeconds == null) return;
  if (time < introStartSeconds || time >= introEndSeconds - 0.25) return;
  introAutoSkipped = true;
  skipIntro(player);
}

function updateSkipIntroVisibility(time, btn) {
  if (!btn) return;
  const visible =
    introStartSeconds != null &&
    introEndSeconds != null &&
    time >= introStartSeconds &&
    time < introEndSeconds - 0.25;
  btn.hidden = !visible;
}

function applyIntroFromCues() {
  const cues = cuesEn.length ? cuesEn : cuesFr;
  const range = detectIntroRangeSeconds(cues);
  introStartSeconds = range ? range.start : null;
  introEndSeconds = range ? range.end : null;
}

async function requestFullscreen(el) {
  if (!el || document.fullscreenElement) return;
  try {
    if (typeof el.requestFullscreen === "function") {
      await el.requestFullscreen();
    } else if (typeof el.webkitRequestFullscreen === "function") {
      el.webkitRequestFullscreen();
    }
  } catch {
    // Browser may block without a fresh user gesture (e.g. after long prep).
  }
}

async function togglePlayerFullscreen(frame) {
  if (!frame) return;
  if (document.fullscreenElement === frame || document.webkitFullscreenElement === frame) {
    await exitFullscreen();
    return;
  }
  await requestFullscreen(frame);
}

function syncFullscreenButton() {
  const btn = document.getElementById("toggle-fullscreen");
  if (!btn) return;
  const shell = document.getElementById("player-shell");
  const active =
    Boolean(shell) &&
    (document.fullscreenElement === shell || document.webkitFullscreenElement === shell);
  btn.classList.toggle("is-active", active);
  btn.textContent = active ? "Quitter le plein écran" : "Plein écran";
  btn.title = active ? "Quitter le plein écran" : "Plein écran";
}

async function exitFullscreen() {
  if (!document.fullscreenElement && !document.webkitFullscreenElement) return;
  try {
    if (typeof document.exitFullscreen === "function") {
      await document.exitFullscreen();
    } else if (typeof document.webkitExitFullscreen === "function") {
      document.webkitExitFullscreen();
    }
  } catch {
    // ignore
  }
}

function isPlaybackReady(status) {
  // Video alone is not enough: Open must wait for the same prep as "Préparer"
  // (subtitles extracted or generated). Partial EN-or-FR is acceptable.
  return (
    status?.video?.status === "ready" &&
    status?.subtitles?.status !== "missing"
  );
}

async function ensureReady(path) {
  let status = await api(`/api/status?path=${encodeURIComponent(path)}`);
  if (isPlaybackReady(status)) {
    hidePrepOverlay();
    return status;
  }

  showPrepOverlay("Démarrage de la préparation…", 0);
  await api("/api/prepare-video", {
    method: "POST",
    body: JSON.stringify({ path }),
  });

  for (;;) {
    status = await api(`/api/status?path=${encodeURIComponent(path)}`);
    if (status.video?.status === "error") {
      throw new Error(
        (status.video.message && String(status.video.message).trim()) ||
          "Échec de préparation",
      );
    }
    if (isPlaybackReady(status)) return status;
    const pct = typeof status.video?.progress === "number" ? status.video.progress : null;
    const message =
      status.video?.message ||
      (pct == null ? "Préparation en cours…" : `Préparation ${pct} %`);
    showPrepOverlay(message, pct);
    await sleep(700);
  }
}

async function loadSubtitles(path) {
  cuesEn = [];
  cuesFr = [];
  const toggleEn = document.getElementById("toggle-en");
  const toggleFr = document.getElementById("toggle-fr");
  try {
    const enText = await fetchText(`/api/subtitles.vtt?path=${encodeURIComponent(path)}&lang=en`);
    cuesEn = parseVtt(enText);
  } catch {
    if (toggleEn) toggleEn.disabled = true;
  }
  try {
    const frText = await fetchText(`/api/subtitles.vtt?path=${encodeURIComponent(path)}&lang=fr`);
    cuesFr = parseVtt(frText);
  } catch {
    if (toggleFr) toggleFr.disabled = true;
  }
}

async function fetchText(url) {
  let res;
  try {
    res = await fetch(url);
  } catch (err) {
    reportConnectionLost();
    throw err;
  }
  if (!connectionOnline) {
    reportConnectionOk({ reloadIfNeeded: false });
  }
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.text();
}

function updateCueDisplay(time) {
  const subEn = document.getElementById("sub-en");
  const subFr = document.getElementById("sub-fr");
  if (!subEn || !subFr) return;
  const en = showEn ? cuesEn.find((c) => time >= c.start && time < c.end) : null;
  const fr = showFr ? cuesFr.find((c) => time >= c.start && time < c.end) : null;
  subEn.textContent = en?.text || "";
  subEn.classList.toggle("is-empty", !en?.text);
  subFr.textContent = fr?.text || "";
  subFr.classList.toggle("is-empty", !fr?.text);
  subFr.hidden = !showFr;
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * @param {{ fromReconnect?: boolean }} [opts]
 */
async function boot(opts = {}) {
  if (bootInFlight) return;
  bootInFlight = true;
  startConnectionMonitor();
  try {
    const [library, settings] = await Promise.all([
      api("/api/library"),
      api("/api/settings").catch(() => ({ skipIntro: false, purgeCacheAfterWatch: false })),
    ]);
    catalog = library;
    rebuildSlugs(catalog);
    autoSkipIntro = Boolean(settings.skipIntro);
    purgeCacheAfterWatch = Boolean(settings.purgeCacheAfterWatch);
    applySubtitleSizes();

    applyWatchModeFromLocation();
    const resolved = navFromLocation(catalog);
    if (!resolved) {
      nav = { page: "home" };
      history.replaceState({}, "", "/");
    } else {
      nav = resolved;
      const canonical = pathForNav(nav);
      const current = `${window.location.pathname}${window.location.search}`;
      if (nav.page !== "not-found" && canonical !== current) {
        history.replaceState({}, "", canonical);
      }
    }
    pendingFullscreen = false;
    bindBrandHome();
    render();
    syncPresenceWithNav();
  } catch (err) {
    reportConnectionLost();
    if (!opts.fromReconnect || catalog == null) {
      main.innerHTML = `<p class="muted">Impossible de joindre le serveur.</p>`;
    }
  } finally {
    bootInFlight = false;
  }
}

window.addEventListener("popstate", () => {
  applyLocationFromHistory();
});

window.addEventListener("pagehide", () => {
  stopPresenceLoop();
});

window.addEventListener("keydown", (event) => {
  if (nav.page !== "watch") return;
  const target = event.target;
  if (
    target instanceof HTMLElement &&
    (target.tagName === "INPUT" || target.tagName === "SELECT" || target.tagName === "TEXTAREA")
  ) {
    return;
  }
  const player = document.getElementById("player");
  const shell = document.getElementById("player-shell");
  if (!player) return;
  if (shell) revealChrome(shell, player);
  if (event.code === "Space") {
    event.preventDefault();
    void togglePlayPause(player);
  } else if (event.code === "Home") {
    event.preventDefault();
    seekToStart(player);
  } else if (event.code === "ArrowLeft") {
    event.preventDefault();
    seekBy(player, -5);
  } else if (event.code === "ArrowRight") {
    event.preventDefault();
    seekBy(player, 5);
  } else if (event.code === "KeyF") {
    if (shell) void togglePlayerFullscreen(shell);
  }
});

void boot();
