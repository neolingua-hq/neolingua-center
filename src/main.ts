import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  addMediaRoot,
  loadCatalog,
  loadMediaRoots,
  loadNetworkInfo,
  loadPresence,
  loadServerStatus,
  loadSettings,
  nextStep,
  openWatchLauncher,
  persistSettings,
  pickMediaDirectory,
  prepareEpisode,
  prepareEpisodes,
  cancelPrepare,
  prevStep,
  removeMediaRoot,
  scanCatalog,
  STEP_LABELS,
  STEP_ORDER,
  stepIndex,
  type AppSettings,
  type CatalogEpisode,
  type CatalogMovie,
  type CatalogSeries,
  type CatalogSnapshot,
  type EpisodePrep,
  type MediaRoot,
  type NetworkInfo,
  type PresenceSession,
  type ServerStatus,
  type WizardStep,
} from "./settings";
import {
  checkForAppUpdate,
  getAvailableUpdateVersion,
  maybeCheckUpdatesOnBoot,
  onUpdateStateChange,
  startHourlyUpdateChecks,
} from "./updater";
import {
  buildSlugById,
  catalogItemCount,
  type CenterRoute,
  displayName,
  escapeHtml,
  folderDisplayName,
  formatPresenceLabel as formatPresenceLabelPure,
  formatPrepMessage,
  formatTrackConstitution,
  isCatalogSnapshot,
  itemMatchesLibrarySearch as itemMatchesLibrarySearchQuery,
  locationLooksLikeCatalog as locationLooksLikeCatalogPath,
  normalizePathname as normalizePathnameInput,
  normalizeQuizInterval,
  normalizeSettings,
  pathForRoute as pathForRoutePure,
  routeFromPathname,
  routeFromSettings as routeFromSettingsPure,
  safeMediaUrl,
  slugify,
  trackSourceTooltip,
} from "./catalogLogic";

const main = document.getElementById("main")!;

let settings: AppSettings | null = null;
let mediaRoots: MediaRoot[] = [];
let networkInfo: NetworkInfo | null = null;
let serverStatus: ServerStatus | null = null;
let catalog: CatalogSnapshot | null = null;
let catalogBusy = false;
let catalogForcePending = false;
let catalogError = "";
/** Generation token to ignore stale async catalog results. */
let catalogGeneration = 0;
/** Live filter on the library home grid. */
let librarySearchQuery = "";
/** Deferred scan: folders chosen in the wizard, run when arriving at the library. */
let pendingCatalogScan = false;

let presenceSessions: PresenceSession[] = [];
let statusBarTimer: ReturnType<typeof setInterval> | null = null;
const STATUS_BAR_POLL_MS = 2500;
/** Prevent overlapping status bar refreshes. */
let statusBarRefreshInFlight = false;
let appVersion = "";

let currentRoute: CenterRoute = { view: "setup", step: "welcome" };

function folderIconSvg(): string {
  return `<svg class="media-root-icon" width="22" height="22" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    <path fill="currentColor" d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8l-2-2z"/>
  </svg>`;
}

function renderMediaRootsList(): string {
  if (mediaRoots.length === 0) {
    return `<p class="path-box is-empty">Aucun dossier ajouté</p>`;
  }
  return `<ul class="media-root-list" id="media-root-list">
    ${mediaRoots
      .map((root) => {
        const name = folderDisplayName(root.path);
        const ok = root.accessible !== false;
        const statusLabel = ok ? "Accessible" : "Inaccessible";
        return `
      <li class="media-root-item ${ok ? "is-ok" : "is-missing"}">
        <div class="media-root-icon-wrap" aria-hidden="true">${folderIconSvg()}</div>
        <div class="media-root-body">
          <div class="media-root-name">${escapeHtml(name)}</div>
          <div class="media-root-path" title="${escapeHtml(root.path)}">${escapeHtml(root.path)}</div>
          <div class="media-root-status ${ok ? "is-ok" : "is-missing"}">${statusLabel}</div>
        </div>
        <button type="button" class="media-root-remove" data-id="${root.id}" aria-label="Retirer ${escapeHtml(name)}">
          <svg width="18" height="18" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
            <path fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" d="M4 7h16M9 7V5h6v2m-8 0l1 12h8l1-12"/>
          </svg>
        </button>
      </li>`;
      })
      .join("")}
  </ul>`;
}

function renderLibraryCheckRow(): string {
  const current = Number(settings?.libraryCheckMinutes ?? 60);
  const options = [
    [0, "Désactivé"],
    [15, "15 minutes"],
    [30, "30 minutes"],
    [60, "1 heure"],
    [120, "2 heures"],
    [360, "6 heures"],
    [1440, "24 heures"],
  ];
  return `
    <div class="media-scan-row">
      <label class="media-scan-label" for="library-check">Analyse automatique</label>
      <select class="field-input field-input--compact" id="library-check">
        ${options
          .map(
            ([value, label]) =>
              `<option value="${value}" ${current === Number(value) ? "selected" : ""}>${label}</option>`,
          )
          .join("")}
      </select>
    </div>
  `;
}

function renderStepDots(current: WizardStep): string {
  const idx = stepIndex(current);
  return STEP_ORDER.map((step, i) => {
    let cls = "wizard-step-dot";
    let state = "à venir";
    if (i === idx) {
      cls += " is-active";
      state = "étape en cours";
    } else if (i < idx) {
      cls += " is-done";
      state = "terminée";
    }
    const label = STEP_LABELS[step];
    const aria = escapeHtml(`${label} (${state})`);
    return [
      `<button type="button" class="${cls}" data-setup-step="${step}"`,
      ` title="${escapeHtml(label)}" aria-label="${aria}"></button>`,
    ].join("");
  }).join("");
}

function renderNetworkBody(): string {
  if (!settings || !networkInfo) {
    return `
      <h1>Réseau</h1>
      <p>Ouvrez cette adresse dans un navigateur (TV, téléphone ou cet ordinateur) pour regarder en solo.</p>
      <p class="muted">Chargement des adresses…</p>
    `;
  }

  const port = effectiveServerPort();
  const primary = networkInfo.addresses[0] ?? networkInfo.localhost;
  const accessUrl = `http://${primary}:${port}/`;
  const serverHint = serverStatus?.error
    ? `<p class="media-hint">${escapeHtml(serverStatus.error)}</p>`
    : serverStatus?.running && settings.serverPort !== serverStatus.port
      ? `<p class="muted">Port ${serverStatus.port} utilisé (le ${settings.serverPort} était déjà pris).</p>`
      : "";

  const hostCard = networkInfo.hostName
    ? `
    <div class="settings-item">
      <div class="settings-item-text">
        <span class="settings-item-title">Nom sur le réseau</span>
        <span class="settings-item-desc settings-item-mono">${escapeHtml(networkInfo.hostName)}</span>
      </div>
    </div>`
    : "";

  const addressList =
    networkInfo.addresses.length > 0
      ? networkInfo.addresses
          .map((addr) => `<span class="network-chip">${escapeHtml(addr)}</span>`)
          .join("")
      : `<span class="muted">Aucune adresse détectée</span>`;

  return `
    <h1>Réseau</h1>
    <p>Ouvrez cette adresse dans un navigateur (TV, téléphone ou cet ordinateur) pour regarder en solo.</p>
    ${serverHint}
    <div class="network-access">
      <div class="network-access-label">Adresse à ouvrir</div>
      <div class="network-access-row">
        <code class="network-access-url" id="network-url">${escapeHtml(accessUrl)}</code>
        <button type="button" class="settings-key-toggle" id="copy-network-url">Copier</button>
      </div>
    </div>
    <div class="settings-list">
      ${hostCard}
      <div class="settings-item settings-item--stack">
        <div class="settings-item-text">
          <span class="settings-item-title">Adresses IP</span>
          <span class="settings-item-desc">Utilisez-en une si le nom ne répond pas.</span>
        </div>
        <div class="network-chip-row">${addressList}</div>
      </div>
      <div class="settings-item">
        <div class="settings-item-text">
          <span class="settings-item-title">Port</span>
          <span class="settings-item-desc">À changer seulement en cas de conflit.</span>
        </div>
        <input
          class="field-input field-input--port"
          type="number"
          id="network-port"
          min="1024"
          max="65535"
          step="1"
          value="${port}"
          aria-label="Port"
        />
      </div>
    </div>
  `;
}

function renderWizard(): void {
  if (!settings) return;

  const step = settings.wizardStep;
  let body = "";

  switch (step) {
    case "welcome":
      body = `
        <h1>Bienvenue</h1>
        <p>Configurez Neolingua Center sur cette machine. Le visionnage se fera sur la TV ou un navigateur du réseau local.</p>
      `;
      break;
    case "media":
      body = `
        <h1>Dossiers des vidéos</h1>
        <p>Ajoutez un ou plusieurs dossiers. Center y détecte les séries (fichiers S01E01, etc.) et les films.</p>
        <div class="media-editor">
          ${renderMediaRootsList()}
          <button type="button" class="media-add" id="pick-media">
            <span class="media-add-plus" aria-hidden="true">+</span>
            Ajouter un dossier…
          </button>
        </div>
        <p class="muted media-hint" id="media-hint" hidden></p>
        ${renderLibraryCheckRow()}
      `;
      break;
    case "options":
      body = `
        <h1>Options</h1>
        <p>Réglages par défaut pour la lecture, les Jam et le catalogue.</p>
        <section class="settings-section">
          <h2 class="settings-section-title">Lecture et cache</h2>
          <div class="settings-list">
            <label class="settings-item settings-item--toggle" for="opt-skip-intro">
              <div class="settings-item-text">
                <span class="settings-item-title">Passer le générique</span>
                <span class="settings-item-desc">Saute automatiquement le générique des épisodes quand c’est possible.</span>
              </div>
              <input type="checkbox" id="opt-skip-intro" ${settings.skipIntro ? "checked" : ""} />
            </label>
            <div class="settings-item">
              <div class="settings-item-text">
                <span class="settings-item-title">Taille du cache</span>
                <span class="settings-item-desc">Espace pour les fichiers prêts à la lecture (vidéo convertie et sous-titres). Les originaux ne sont jamais touchés.</span>
              </div>
              <select class="field-input field-input--compact" id="opt-cache-max" aria-label="Taille maximale du cache">
                ${(() => {
                  const currentGb = Number(settings?.cacheMaxGb ?? 20);
                  return [0, 5, 10, 20, 50, 100]
                    .map((gb) => {
                      const label = gb === 0 ? "Illimité" : `${gb} Go`;
                      const selected = currentGb === gb ? "selected" : "";
                      return `<option value="${gb}" ${selected}>${label}</option>`;
                    })
                    .join("");
                })()}
              </select>
            </div>
            <label class="settings-item settings-item--toggle" for="opt-purge-after-watch">
              <div class="settings-item-text">
                <span class="settings-item-title">Purger après visionnage</span>
                <span class="settings-item-desc">Supprime le cache d’un média une fois regardé jusqu’à la fin.</span>
              </div>
              <input type="checkbox" id="opt-purge-after-watch" ${
                settings.purgeCacheAfterWatch ? "checked" : ""
              } />
            </label>
          </div>
        </section>
        <section class="settings-section">
          <h2 class="settings-section-title">Jam</h2>
          <div class="settings-list">
            <label class="settings-item settings-item--toggle" for="opt-jam-quiz">
              <div class="settings-item-text">
                <span class="settings-item-title">Mode Quiz</span>
                <span class="settings-item-desc">Par défaut, les nouveaux Jam proposent des trous de vocabulaire. Chaque Jam peut le changer.</span>
              </div>
              <input type="checkbox" id="opt-jam-quiz" ${settings.jamQuizMode ? "checked" : ""} />
            </label>
            <div class="settings-item">
              <div class="settings-item-text">
                <span class="settings-item-title">Fréquence des quiz</span>
                <span class="settings-item-desc">Intervalle cible entre deux pauses quiz (défaut hôte).</span>
              </div>
              <select class="field-input field-input--compact" id="opt-jam-quiz-interval" aria-label="Fréquence des quiz">
                ${(() => {
                  const current = Number(settings?.jamQuizIntervalSeconds ?? 60);
                  return [
                    [30, "30 secondes"],
                    [60, "1 minute"],
                    [120, "2 minutes"],
                    [300, "5 minutes"],
                    [900, "15 minutes"],
                  ]
                    .map(([seconds, label]) => {
                      const selected = current === seconds ? "selected" : "";
                      return `<option value="${seconds}" ${selected}>${label}</option>`;
                    })
                    .join("");
                })()}
              </select>
            </div>
            <label class="settings-item settings-item--toggle" for="opt-jam-display-sub-en">
              <div class="settings-item-text">
                <span class="settings-item-title">Sous-titres écran · anglais</span>
                <span class="settings-item-desc">Par défaut sur l’écran vidéo d’un Jam (en plus du téléphone). Chaque Jam peut le changer.</span>
              </div>
              <input type="checkbox" id="opt-jam-display-sub-en" ${
                settings.jamDisplaySubEn ? "checked" : ""
              } />
            </label>
            <label class="settings-item settings-item--toggle" for="opt-jam-display-sub-fr">
              <div class="settings-item-text">
                <span class="settings-item-title">Sous-titres écran · français</span>
                <span class="settings-item-desc">Par défaut sur l’écran vidéo d’un Jam. Sur une phrase quiz, l’autre langue est masquée.</span>
              </div>
              <input type="checkbox" id="opt-jam-display-sub-fr" ${
                settings.jamDisplaySubFr ? "checked" : ""
              } />
            </label>
          </div>
        </section>
        <section class="settings-section">
          <h2 class="settings-section-title">Catalogue</h2>
          <div class="settings-list">
            <div class="settings-item settings-item--stack">
              <div class="settings-item-text">
                <span class="settings-item-title">The MovieDB</span>
                <span class="settings-item-desc">Affiches et regroupement des séries. Vous pouvez laisser vide.</span>
              </div>
              <div class="settings-key-row">
                <input
                  class="field-input settings-key-input"
                  type="password"
                  id="tmdb-key"
                  autocomplete="off"
                  spellcheck="false"
                  placeholder="Clé API"
                  value=""
                  data-has-saved-key="${settings.tmdbApiKey ? "true" : "false"}"
                />
                <button type="button" class="settings-key-toggle" id="tmdb-toggle-vis" aria-pressed="false">
                  Afficher
                </button>
              </div>
              ${settings.tmdbApiKey ? `<p class="settings-key-hint">Clé enregistrée (••••${escapeHtml(settings.tmdbApiKey.slice(-4))}). Videz le champ pour la supprimer.</p>` : ""}
            </div>
          </div>
        </section>
      `;
      break;
    case "network":
      body = renderNetworkBody();
      break;
  }

  const isLast = step === "network";
  const showBack = step !== "welcome";
  const mediaBlocked = step === "media" && mediaRoots.length === 0;

  main.innerHTML = `
    <section class="wizard" aria-label="Assistant de configuration">
      <div class="wizard-card ${
        step === "media" || step === "options" || step === "network" ? "wizard-card--fit" : ""
      }">${body}</div>
      <nav class="wizard-steps" aria-label="Progression">${renderStepDots(step)}</nav>
      <div class="actions">
        ${
          showBack
            ? `<button type="button" class="ghost" id="wizard-back">Retour</button>`
            : ""
        }
        ${
          isLast
            ? `<button type="button" class="primary" id="wizard-next">Voir la bibliothèque</button>`
            : `<button type="button" class="primary" id="wizard-next" ${mediaBlocked ? "disabled" : ""}>Continuer</button>`
        }
      </div>
    </section>
  `;

  document.getElementById("pick-media")?.addEventListener("click", onPickMedia);
  document.querySelectorAll(".media-root-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = Number((btn as HTMLElement).dataset.id);
      if (!Number.isFinite(id)) return;
      const root = mediaRoots.find((r) => r.id === id);
      const name = root ? folderDisplayName(root.path) : "ce dossier";
      if (!window.confirm(`Retirer « ${name} » de la liste ?`)) return;
      void onRemoveMedia(id);
    });
  });
  document.getElementById("wizard-back")?.addEventListener("click", onBack);
  document.getElementById("wizard-next")?.addEventListener("click", onNext);
  document.querySelectorAll("[data-setup-step]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const step = (btn as HTMLElement).dataset.setupStep as WizardStep | undefined;
      if (!step || !STEP_ORDER.includes(step) || !settings) return;
      if (step === settings.wizardStep) return;
      readOptionsFromDom();
      settings = { ...settings, wizardStep: step, setupComplete: false };
      currentRoute = { view: "setup", step };
      void saveAndRender({ replace: false });
    });
  });

  document.getElementById("tmdb-toggle-vis")?.addEventListener("click", () => {
    const input = document.getElementById("tmdb-key") as HTMLInputElement | null;
    const btn = document.getElementById("tmdb-toggle-vis") as HTMLButtonElement | null;
    if (!input || !btn) return;
    const show = input.type === "password";
    input.type = show ? "text" : "password";
    btn.textContent = show ? "Masquer" : "Afficher";
    btn.setAttribute("aria-pressed", show ? "true" : "false");
  });

  // Track if user explicitly clears the TMDB key field.
  const tmdbInput = document.getElementById("tmdb-key") as HTMLInputElement | null;
  tmdbInput?.addEventListener("input", () => {
    if (tmdbInput.value.trim() === "" && tmdbInput.dataset.hasSavedKey === "true") {
      tmdbInput.dataset.userCleared = "true";
    } else {
      delete tmdbInput.dataset.userCleared;
    }
  });

  const jamQuiz = document.getElementById("opt-jam-quiz") as HTMLInputElement | null;
  const jamQuizInterval = document.getElementById(
    "opt-jam-quiz-interval",
  ) as HTMLSelectElement | null;
  const syncJamQuizInterval = () => {
    if (!jamQuizInterval) return;
    jamQuizInterval.disabled = !(jamQuiz?.checked ?? false);
  };
  jamQuiz?.addEventListener("change", syncJamQuizInterval);
  syncJamQuizInterval();

  document.getElementById("copy-network-url")?.addEventListener("click", () => {
    void copyNetworkUrl();
  });

  const portInput = document.getElementById("network-port") as HTMLInputElement | null;
  portInput?.addEventListener("input", updateNetworkExample);
}

function lanBaseUrl(): string {
  if (!settings || !networkInfo) return "";
  const primary = networkInfo.addresses[0] ?? networkInfo.localhost;
  return `http://${primary}:${effectiveServerPort()}`;
}

function statusBarAccessUrl(): string {
  const base = lanBaseUrl();
  if (!base) return "";
  return base.endsWith("/") ? base : `${base}/`;
}

function formatPresenceLabel(sessions: PresenceSession[]): string {
  return formatPresenceLabelPure(sessions, Boolean(serverStatus?.running));
}

function renderStatusBar(): void {
  const urlEl = document.getElementById("status-bar-url");
  const playbackEl = document.getElementById("status-bar-playback");
  const dotEl = document.getElementById("status-bar-dot");
  const serverDotEl = document.getElementById("status-bar-server-dot");
  const serverLabelEl = document.getElementById("status-bar-server-label");
  const versionEl = document.getElementById("status-bar-version");
  const updateBtn = document.getElementById("status-bar-update") as HTMLButtonElement | null;
  const copyBtn = document.getElementById("status-bar-copy-url") as HTMLButtonElement | null;
  const openBtn = document.getElementById("status-bar-open-url") as HTMLButtonElement | null;
  const salonCluster = document.querySelector(".status-bar-cluster--salon");
  if (!urlEl || !playbackEl) return;

  const serverOnline = Boolean(serverStatus?.running);
  salonCluster?.classList.toggle("is-online", serverOnline);
  salonCluster?.classList.toggle("is-offline", !serverOnline);
  serverDotEl?.classList.toggle("is-online", serverOnline);
  if (serverLabelEl) {
    if (serverOnline) {
      serverLabelEl.textContent = "En ligne";
    } else if (serverStatus?.error?.trim()) {
      serverLabelEl.textContent = "Hors ligne";
    } else {
      serverLabelEl.textContent = "Connexion…";
    }
  }

  const url = statusBarAccessUrl();
  if (!url) {
    urlEl.textContent = serverStatus?.error
      ? "Serveur indisponible"
      : "Adresse en cours de détection…";
  } else if (serverStatus && !serverStatus.running) {
    urlEl.textContent = serverStatus.error?.trim() || "Serveur arrêté";
  } else {
    urlEl.textContent = url;
  }
  const urlReady = Boolean(url) && !(serverStatus && !serverStatus.running);
  if (copyBtn) {
    copyBtn.disabled = !urlReady;
    if (!copyBtn.classList.contains("is-copied")) {
      copyBtn.title = copyBtn.disabled ? "Adresse indisponible" : "Copier l’adresse";
    }
  }
  if (openBtn) {
    openBtn.disabled = !urlReady;
    openBtn.title = openBtn.disabled
      ? "Adresse indisponible"
      : "Ouvrir dans le navigateur";
  }

  const isLive = presenceSessions.some((s) => s.playing);
  playbackEl.innerHTML = formatPresenceLabel(presenceSessions);
  playbackEl.classList.toggle("is-active", isLive);
  dotEl?.classList.toggle("is-live", isLive);

  const updateVersion = getAvailableUpdateVersion();
  if (versionEl) {
    versionEl.textContent = appVersion ? `v${appVersion}` : "";
    versionEl.hidden = !appVersion;
    versionEl.classList.toggle("is-outdated", Boolean(updateVersion));
    if (updateVersion) {
      versionEl.title = `Mise à jour ${updateVersion} disponible`;
      versionEl.setAttribute(
        "aria-label",
        `Version ${appVersion}, mise à jour ${updateVersion} disponible`,
      );
    } else {
      versionEl.title = "Vérifier les mises à jour";
      versionEl.setAttribute("aria-label", "Vérifier les mises à jour");
    }
  }

  if (updateBtn) {
    updateBtn.hidden = !updateVersion;
    if (updateVersion) {
      updateBtn.title = `Mise à jour ${updateVersion} disponible`;
      updateBtn.setAttribute(
        "aria-label",
        `Mise à jour ${updateVersion} disponible`,
      );
    }
  }
}

async function loadAppVersion(): Promise<void> {
  try {
    appVersion = await getVersion();
  } catch {
    appVersion = "";
  }
}

async function onStatusBarUpdateClick(): Promise<void> {
  const updateBtn = document.getElementById("status-bar-update") as HTMLButtonElement | null;
  const versionBtn = document.getElementById("status-bar-version") as HTMLButtonElement | null;
  if (updateBtn) updateBtn.disabled = true;
  if (versionBtn) {
    versionBtn.disabled = true;
    versionBtn.classList.add("is-checking");
  }
  try {
    const result = await checkForAppUpdate({ force: true });
    if (versionBtn && result.status === "upToDate") {
      versionBtn.title = "À jour";
      window.setTimeout(() => {
        if (!getAvailableUpdateVersion()) {
          versionBtn.title = "Vérifier les mises à jour";
        }
      }, 2000);
    }
  } finally {
    if (updateBtn) updateBtn.disabled = false;
    if (versionBtn) {
      versionBtn.disabled = false;
      versionBtn.classList.remove("is-checking");
    }
    renderStatusBar();
  }
}

async function refreshPresence(): Promise<void> {
  try {
    const snap = await loadPresence();
    presenceSessions = Array.isArray(snap.sessions) ? snap.sessions : [];
  } catch {
    presenceSessions = [];
  }
}

async function refreshStatusBar(): Promise<void> {
  // Skip if a previous refresh is still in flight.
  if (statusBarRefreshInFlight) return;
  statusBarRefreshInFlight = true;
  try {
    if (!networkInfo) {
      try {
        networkInfo = await loadNetworkInfo();
      } catch {
        networkInfo = null;
      }
    }
    await refreshServerStatus();
    await refreshPresence();
    renderStatusBar();
  } finally {
    statusBarRefreshInFlight = false;
  }
}

function ensureStatusBarLoop(): void {
  if (statusBarTimer) return;
  const copyBtn = document.getElementById("status-bar-copy-url");
  copyBtn?.addEventListener("click", () => {
    void copyStatusBarUrl();
  });
  document.getElementById("status-bar-open-url")?.addEventListener("click", () => {
    void openStatusBarUrl();
  });
  document.getElementById("status-bar-update")?.addEventListener("click", () => {
    void onStatusBarUpdateClick();
  });
  document.getElementById("status-bar-version")?.addEventListener("click", () => {
    void onStatusBarUpdateClick();
  });
  onUpdateStateChange(() => {
    renderStatusBar();
  });
  void (async () => {
    await loadAppVersion();
    await refreshStatusBar();
  })();
  statusBarTimer = window.setInterval(() => {
    void refreshStatusBar();
  }, STATUS_BAR_POLL_MS);
}

async function copyStatusBarUrl(): Promise<void> {
  const urlEl = document.getElementById("status-bar-url");
  const btn = document.getElementById("status-bar-copy-url");
  const text = statusBarAccessUrl() || urlEl?.textContent?.trim();
  if (!text || !text.startsWith("http") || !btn) return;
  try {
    await writeText(text);
  } catch {
    await navigator.clipboard.writeText(text);
  }
  const prevTitle = btn.getAttribute("title") || "Copier l’adresse";
  btn.classList.add("is-copied");
  btn.title = "Copié";
  window.setTimeout(() => {
    btn.classList.remove("is-copied");
    btn.title = prevTitle;
  }, 1200);
}

async function openStatusBarUrl(): Promise<void> {
  const url = statusBarAccessUrl();
  if (!url || !url.startsWith("http")) return;
  if (serverStatus && !serverStatus.running) {
    const detail =
      serverStatus.error?.trim() ||
      "Le serveur de lecture n'est pas démarré. Vérifiez le port dans la configuration.";
    window.alert(detail);
    return;
  }
  try {
    await openWatchLauncher(url);
  } catch (err) {
    console.error(err);
    window.alert(
      err instanceof Error ? err.message : "Impossible d’ouvrir le navigateur.",
    );
  }
}

function effectiveServerPort(): number {
  if (serverStatus?.running) return serverStatus.port;
  return settings?.serverPort || 8787;
}

function seriesSlugByIdMap(): Map<string, string> {
  return buildSlugById(catalog?.series ?? [], (s) => displayName(s));
}

function movieSlugByIdMap(): Map<string, string> {
  return buildSlugById(catalog?.movies ?? [], (m) => displayName(m));
}

function episodeWatchHref(
  series: CatalogSeries,
  episode: CatalogEpisode,
  mode: "solo" | "jam" = "solo",
): string {
  const slug = seriesSlugByIdMap().get(series.id) || slugify(displayName(series));
  const ep = String(episode.episode).padStart(2, "0");
  const base = `/series/${encodeURIComponent(slug)}/s${episode.season}/e${ep}`;
  return `${base}?mode=${mode}`;
}

function movieWatchHref(movie: CatalogMovie, mode: "solo" | "jam" = "solo"): string {
  const slug = movieSlugByIdMap().get(movie.id) || slugify(displayName(movie));
  return `/movies/${encodeURIComponent(slug)}?mode=${mode}`;
}

function seriesSlug(seriesId: string): string {
  return seriesSlugByIdMap().get(seriesId) || slugify(seriesId);
}

function movieSlug(movieId: string): string {
  return movieSlugByIdMap().get(movieId) || slugify(movieId);
}

function findSeriesByRef(ref: string): CatalogSeries | undefined {
  const decoded = decodeURIComponent(ref);
  const bySlug = [...seriesSlugByIdMap().entries()].find(([, slug]) => slug === decoded);
  if (bySlug) return catalog?.series.find((s) => s.id === bySlug[0]);
  return catalog?.series.find((s) => s.id === decoded);
}

function findMovieByRef(ref: string): CatalogMovie | undefined {
  const decoded = decodeURIComponent(ref);
  const bySlug = [...movieSlugByIdMap().entries()].find(([, slug]) => slug === decoded);
  if (bySlug) return catalog?.movies.find((m) => m.id === bySlug[0]);
  return catalog?.movies.find((m) => m.id === decoded);
}

function normalizePathname(): string {
  return normalizePathnameInput(window.location.pathname);
}

function pathForRoute(route: CenterRoute): string {
  return pathForRoutePure(route, seriesSlug, movieSlug);
}

/** null = use settings / default (/, empty, unknown junk). */
function routeFromLocation(): CenterRoute | null {
  return routeFromPathname(window.location.pathname, findSeriesByRef, findMovieByRef);
}

function routeFromSettings(): CenterRoute {
  return routeFromSettingsPure(settings);
}

function syncHistory(route: CenterRoute, replace: boolean): void {
  const url = pathForRoute(route);
  const current = normalizePathname();
  if (url === current) return;
  if (replace) history.replaceState({ route }, "", url);
  else history.pushState({ route }, "", url);
}

function goToRoute(route: CenterRoute, opts: { replace?: boolean } = {}): void {
  currentRoute = route;
  syncHistory(route, Boolean(opts.replace));
  if (route.view === "setup") {
    if (settings) {
      settings = { ...settings, wizardStep: route.step, setupComplete: false };
    }
    void renderWizardAsync();
    return;
  }
  if (settings && !settings.setupComplete) {
    settings = { ...settings, setupComplete: true };
  }
  renderCockpit();
}

function absoluteWatchUrl(href: string): string {
  return `${lanBaseUrl()}${href}`;
}

function playWatchButtons(hrefSolo: string, hrefJam: string): string {
  return `<button type="button" class="episode-prep-action episode-prep-action--jam" data-open-watch="${escapeHtml(hrefJam)}" title="Lancer un Jam dans le navigateur">Jam</button><button type="button" class="episode-prep-action episode-prep-action--lire" data-open-watch="${escapeHtml(hrefSolo)}" title="Lire dans le navigateur">Lire</button>`;
}

async function openWatchFromHref(href: string, btn?: HTMLElement | null): Promise<void> {
  await refreshServerStatus();
  if (settings && serverStatus?.running && settings.serverPort !== serverStatus.port) {
    settings = { ...settings, serverPort: serverStatus.port };
  }
  if (!serverStatus?.running) {
    const detail =
      serverStatus?.error?.trim() ||
      "Le serveur de lecture n'est pas démarré. Vérifiez le port dans la configuration.";
    window.alert(detail);
    return;
  }
  const url = absoluteWatchUrl(href);
  if (!url.startsWith("http")) return;
  const prev = btn?.textContent;
  if (btn) btn.textContent = "…";
  try {
    await openWatchLauncher(url);
  } catch (err) {
    console.error(err);
    if (btn && prev != null) btn.textContent = prev;
    window.alert(
      err instanceof Error ? err.message : "Impossible d’ouvrir la lecture.",
    );
    return;
  }
  if (btn && prev != null) {
    btn.textContent = "OK";
    window.setTimeout(() => {
      btn.textContent = prev;
    }, 1200);
  }
}

async function copyNetworkUrl(): Promise<void> {
  const urlEl = document.getElementById("network-url");
  const btn = document.getElementById("copy-network-url");
  const text = urlEl?.textContent?.trim();
  if (!text || !btn) return;
  try {
    await writeText(text);
  } catch {
    await navigator.clipboard.writeText(text);
  }
  const prev = btn.textContent;
  btn.textContent = "Copié";
  window.setTimeout(() => {
    btn.textContent = prev;
  }, 1200);
}

function updateNetworkExample(): void {
  if (!settings || !networkInfo) return;
  const portInput = document.getElementById("network-port") as HTMLInputElement | null;
  const urlEl = document.getElementById("network-url");
  if (!portInput || !urlEl) return;
  const port = portInput.value || String(effectiveServerPort());
  const primary = networkInfo.addresses[0] ?? networkInfo.localhost;
  urlEl.textContent = `http://${primary}:${port}/`;
}

function itemMatchesLibrarySearch(item: {
  title: string;
  displayTitle?: string | null;
}): boolean {
  return itemMatchesLibrarySearchQuery(item, librarySearchQuery);
}

function posterMarkup(posterUrl: string | null | undefined, title: string): string {
  const safeUrl = safeMediaUrl(posterUrl);
  if (safeUrl) {
    return `<img class="poster-image" src="${escapeHtml(safeUrl)}" alt="" loading="lazy" />`;
  }
  const letter = title.trim().slice(0, 1).toUpperCase() || "?";
  return `<div class="poster-fallback" aria-hidden="true"><span>${escapeHtml(letter)}</span></div>`;
}

function posterCard(opts: {
  kind: string;
  id: string;
  title: string;
  subtitle?: string;
  posterUrl?: string | null;
  badge?: string;
  extra?: string;
}): string {
  const badge = opts.badge
    ? `<span class="poster-badge">${escapeHtml(opts.badge)}</span>`
    : "";
  return `
    <button type="button" class="poster-card" data-kind="${escapeHtml(opts.kind)}" data-id="${escapeHtml(opts.id)}" ${opts.extra ?? ""}>
      <span class="poster-frame">
        ${posterMarkup(opts.posterUrl, opts.title)}
        ${badge}
      </span>
      <span class="poster-title">${escapeHtml(opts.title)}</span>
      ${opts.subtitle ? `<span class="poster-sub">${escapeHtml(opts.subtitle)}</span>` : ""}
    </button>
  `;
}

function renderLibraryHome(): string {
  const seriesAll = catalog?.series ?? [];
  const moviesAll = catalog?.movies ?? [];
  if (catalogBusy) {
    return `
      <div class="library-loading" role="status" aria-live="polite">
        <div class="library-spinner" aria-hidden="true">
          <span></span><span></span>
        </div>
        <p class="muted">Préparation du catalogue…</p>
      </div>
    `;
  }
  if (catalogError) {
    return `<p class="media-hint">${escapeHtml(catalogError)}</p>`;
  }
  if (seriesAll.length === 0 && moviesAll.length === 0) {
    return `<p class="muted">Aucun média détecté. Ajoutez des dossiers dans la configuration.</p>`;
  }

  const series = seriesAll.filter(itemMatchesLibrarySearch);
  const movies = moviesAll.filter(itemMatchesLibrarySearch);
  const query = librarySearchQuery.trim();

  if (query && series.length === 0 && movies.length === 0) {
    return `<p class="muted">Aucun résultat pour « ${escapeHtml(query)} ».</p>`;
  }

  return `
    ${
      series.length > 0
        ? `<div class="library-section">
            <div class="library-section-head">
              <h2>Séries</h2>
              <span class="library-section-count">${series.length}</span>
            </div>
            <div class="poster-grid">
              ${series
                .map((show) =>
                  posterCard({
                    kind: "series",
                    id: show.id,
                    title: displayName(show),
                    subtitle: `${show.seasons.length} saison${show.seasons.length > 1 ? "s" : ""}`,
                    posterUrl: show.posterUrl,
                  }),
                )
                .join("")}
            </div>
          </div>`
        : ""
    }
    ${
      movies.length > 0
        ? `<div class="library-section">
            <div class="library-section-head">
              <h2>Films</h2>
              <span class="library-section-count">${movies.length}</span>
            </div>
            <div class="poster-grid">
              ${movies
                .map((movie) =>
                  posterCard({
                    kind: "movie",
                    id: movie.id,
                    title: displayName(movie),
                    subtitle: movie.year ? String(movie.year) : undefined,
                    posterUrl: movie.posterUrl,
                  }),
                )
                .join("")}
            </div>
          </div>`
        : ""
    }
  `;
}

function searchIconSvg(): string {
  return `<svg class="library-search-icon" width="18" height="18" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    <circle cx="11" cy="11" r="7" fill="none" stroke="currentColor" stroke-width="1.75"/>
    <path fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" d="M16.5 16.5L21 21"/>
  </svg>`;
}

function searchClearIconSvg(): string {
  return `<svg class="library-search-clear-icon" width="14" height="14" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    <path fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" d="M6 6l12 12M18 6L6 18"/>
  </svg>`;
}

function renderLibrarySearch(): string {
  const hasQuery = librarySearchQuery.trim().length > 0;
  return `
    <div class="library-search${hasQuery ? " has-query" : ""}">
      <label class="visually-hidden" for="library-search">Rechercher dans la bibliothèque</label>
      <span class="library-search-icon-wrap" aria-hidden="true">${searchIconSvg()}</span>
      <input
        type="search"
        id="library-search"
        class="field-input library-search-input"
        placeholder="Rechercher…"
        value="${escapeHtml(librarySearchQuery)}"
        autocomplete="off"
        spellcheck="false"
      />
      <button
        type="button"
        class="library-search-clear"
        id="library-search-clear"
        aria-label="Vider la recherche"
        title="Vider"
        tabindex="-1"
      >${searchClearIconSvg()}</button>
    </div>
  `;
}

function findSeries(id: string): CatalogSeries | undefined {
  return catalog?.series.find((s) => s.id === id);
}

function renderSeriesPage(seriesId: string): string {
  const show = findSeries(seriesId);
  if (!show) return `<p class="muted">Série introuvable.</p>`;
  const seasonCount = show.seasons.length;
  return `
    <div class="library-hero">
      <div class="library-hero-poster">
        <span class="poster-frame poster-frame--hero">
          ${posterMarkup(show.posterUrl, displayName(show))}
        </span>
      </div>
      <div class="library-hero-body">
        <p class="library-meta">${seasonCount} saison${seasonCount > 1 ? "s" : ""}</p>
        ${
          show.synopsis
            ? `<p class="library-synopsis">${escapeHtml(show.synopsis)}</p>`
            : ""
        }
      </div>
    </div>
    <div class="library-section">
      <div class="library-section-head">
        <h2>Saisons</h2>
        <span class="library-section-count">${seasonCount}</span>
      </div>
      <div class="poster-grid">
        ${show.seasons
          .map((season) =>
            posterCard({
              kind: "season",
              id: String(season.number),
              title: season.title?.trim() || `Saison ${season.number}`,
              subtitle: `${season.episodes.length} épisode${season.episodes.length > 1 ? "s" : ""}`,
              posterUrl: season.posterUrl,
              extra: `data-series="${escapeHtml(show.id)}"`,
            }),
          )
          .join("")}
      </div>
    </div>
  `;
}

function renderSeasonPage(seriesId: string, seasonNumber: number): string {
  const show = findSeries(seriesId);
  const season = show?.seasons.find((s) => s.number === seasonNumber);
  if (!show || !season) return `<p class="muted">Saison introuvable.</p>`;
  const episodes = season.episodes;
  const total = episodes.length;
  const readyCount = episodes.filter((ep) => ep.prep?.status === "ready").length;
  const busyCount = episodes.filter((ep) => {
    const status = ep.prep?.status;
    return status === "processing" || status === "queued";
  }).length;
  const pending = episodes.filter((ep) => {
    const status = ep.prep?.status ?? "missing";
    return (
      status !== "ready" &&
      status !== "partial" &&
      status !== "processing" &&
      status !== "queued"
    );
  });
  const pendingCount = pending.length;
  const errorCount = episodes.filter((ep) => ep.prep?.status === "error").length;

  let hint = "";
  if (busyCount > 0) {
    hint = "Préparation en cours, un épisode à la fois.";
  } else if (errorCount > 0) {
    hint =
      errorCount === 1
        ? "Un épisode a échoué. Le détail est sous le titre."
        : `${errorCount} épisodes ont échoué. Le détail est sous chaque titre.`;
  } else if (readyCount === 0) {
    hint = "Rien n'est encore jouable sur le salon.";
  }

  let seasonCta = "";
  if (busyCount > 0) {
    seasonCta = `<button type="button" class="primary" disabled>Préparation en cours…</button>`;
  } else if (pendingCount > 0) {
    const label =
      pendingCount === total
        ? `Préparer les ${pendingCount} épisodes`
        : pendingCount === 1
          ? "Préparer l'épisode restant"
          : `Préparer les ${pendingCount} restants`;
    seasonCta = `<button type="button" class="primary" id="prepare-season">${label}</button>`;
  }

  return `
    <div class="season-ops">
      <div class="season-ops-text">
        <p class="library-meta">${readyCount} / ${total} ${readyCount === 1 ? "prêt" : "prêts"}</p>
        ${hint ? `<p class="season-ops-hint">${escapeHtml(hint)}</p>` : ""}
      </div>
      ${seasonCta}
    </div>
    ${
      season.synopsis
        ? `<details class="season-summary">
            <summary>Résumé de la saison</summary>
            <p>${escapeHtml(season.synopsis)}</p>
          </details>`
        : ""
    }
    <ul class="episode-list">
      ${episodes
        .map((episode) => {
          const title = episode.title?.trim() || episode.label;
          const epNum = String(episode.episode).padStart(2, "0");
          const status = episode.prep?.status ?? "missing";
          const flagsHtml = formatTrackFlags(episode.prep);
          const statusMsg =
            status === "error"
              ? formatPrepMessage(episode.prep?.message) ||
                "La préparation a échoué."
              : "";
          return `
            <li class="episode-row episode-row--${escapeHtml(status)}">
              <span class="episode-num">${escapeHtml(epNum)}</span>
              <div class="episode-meta">
                <span class="episode-title">${escapeHtml(title)}</span>
                <span class="episode-label">${escapeHtml(episode.label)}</span>
                ${flagsHtml}
                ${
                  statusMsg
                    ? `<span class="episode-status-msg episode-status-msg--error">${escapeHtml(statusMsg)}</span>`
                    : ""
                }
              </div>
              ${episodePrepMarkup(show, episode)}
            </li>
          `;
        })
        .join("")}
    </ul>
  `;
}

/** Small EN/FR flags only when the track source is known (native or generated).
 * SVG (not emoji): Windows does not render regional-indicator flag glyphs. */
function formatTrackFlags(prep: EpisodePrep | null | undefined): string {
  const status = prep?.status ?? "missing";
  if (status === "missing" || status === "processing" || status === "queued") {
    return "";
  }
  const chips: string[] = [];
  for (const [lang, flagSrc, source] of [
    ["en", "/flags/gb.svg", prep?.enSource],
    ["fr", "/flags/fr.svg", prep?.frSource],
  ] as const) {
    if (source !== "native" && source !== "generated") continue;
    const tip = trackSourceTooltip(source);
    const generatedNote =
      source === "generated"
        ? `<span class="episode-flag-note">(généré)</span>`
        : "";
    chips.push(
      `<span class="episode-flag episode-flag--${escapeHtml(source)}" title="${escapeHtml(tip)}" aria-label="${escapeHtml(lang.toUpperCase())} : ${escapeHtml(tip)}"><img class="episode-flag-img" src="${flagSrc}" width="18" height="12" alt="" decoding="async" />${generatedNote}</span>`,
    );
  }
  if (chips.length === 0) return "";
  return `<span class="episode-flags" role="group" aria-label="Sous-titres">${chips.join("")}</span>`;
}

function episodePrepMarkup(series: CatalogSeries, episode: CatalogEpisode): string {
  const prep = episode.prep ?? {
    status: "missing",
    video: false,
    subsEn: false,
    subsFr: false,
    enSource: "missing",
    frSource: "missing",
  };
  const status = prep.status;
  const compactMessage =
    status === "missing"
      ? ""
      : formatPrepMessage(prep.message) || formatTrackConstitution(prep);
  const titleAttr = compactMessage
    ? ` title="${escapeHtml(compactMessage)}"`
    : "";
  const playBtns = playWatchButtons(
    episodeWatchHref(series, episode, "solo"),
    episodeWatchHref(series, episode, "jam"),
  );

  // Aligned with the web viewer: Préparer, or Jam + Lire (never both).
  if (status === "ready" || status === "partial") {
    return `<div class="episode-prep episode-prep--row">${playBtns}</div>`;
  }
  if (status === "processing") {
    const pct = Math.max(0, Math.min(100, prep.progress ?? 0));
    const label = prep.message?.trim() || "Préparation…";
    const canceling = /annul/i.test(label);
    return `
      <div class="episode-prep episode-prep--row">
        <button
          type="button"
          class="episode-prep-bar${canceling ? " is-canceling" : ""}"
          aria-valuemin="0"
          aria-valuemax="100"
          aria-valuenow="${pct}"
          aria-label="${escapeHtml(canceling ? "Annulation en cours" : `Préparation ${pct} %, cliquer pour annuler`)}"
          title="${escapeHtml(canceling ? "Annulation…" : "Survolez puis cliquez pour annuler")}"
          data-cancel-prepare="${escapeHtml(episode.path)}"
          data-prep-progress="${escapeHtml(episode.path)}"
        >
          <span class="episode-prep-bar-fill" style="width: ${pct}%"></span>
          <span class="episode-prep-bar-label">${canceling ? "…" : `${pct}%`}</span>
          <span class="episode-prep-bar-cancel" aria-hidden="true">Annuler</span>
        </button>
      </div>
    `;
  }
  if (status === "queued") {
    return `<div class="episode-prep episode-prep--row"><span class="episode-badge episode-badge--busy"${titleAttr}>En file</span></div>`;
  }

  const actionLabel = status === "error" ? "Réessayer" : "Préparer";
  const extraClass = status === "error" ? " episode-prep-action--error" : "";
  return `
    <div class="episode-prep episode-prep--row">
      <button type="button" class="episode-prep-action${extraClass}" data-prepare-path="${escapeHtml(episode.path)}"${titleAttr}>${actionLabel}</button>
    </div>
  `;
}

function applyEpisodePrep(path: string, prep: EpisodePrep): void {
  if (!catalog) return;
  for (const series of catalog.series) {
    for (const season of series.seasons) {
      for (const episode of season.episodes) {
        if (episode.path === path) {
          episode.prep = prep;
          return;
        }
      }
    }
  }
}

function promoteNextQueued(): void {
  if (!catalog || currentRoute.view !== "season") return;
  const nav = currentRoute;
  const show = findSeries(nav.seriesId);
  const season = show?.seasons.find((s) => s.number === nav.season);
  if (!season) return;
  if (season.episodes.some((ep) => ep.prep?.status === "processing")) return;
  const next = season.episodes.find((ep) => ep.prep?.status === "queued");
  if (!next) return;
  applyEpisodePrep(next.path, {
    status: "processing",
    video: next.prep?.video ?? false,
    subsEn: next.prep?.subsEn ?? false,
    subsFr: next.prep?.subsFr ?? false,
    enSource: next.prep?.enSource ?? "missing",
    frSource: next.prep?.frSource ?? "missing",
    message: "Préparation en cours…",
  });
}

function renderMoviePage(movieId: string): string {
  const movie = catalog?.movies.find((m) => m.id === movieId);
  if (!movie) return `<p class="muted">Film introuvable.</p>`;
  return `
    <div class="library-hero">
      <div class="library-hero-poster">
        <span class="poster-frame poster-frame--hero">
          ${posterMarkup(movie.posterUrl, displayName(movie))}
        </span>
      </div>
      <div class="library-hero-body">
        ${movie.year ? `<p class="library-meta">${movie.year}</p>` : ""}
        ${
          movie.synopsis
            ? `<p class="library-synopsis">${escapeHtml(movie.synopsis)}</p>`
            : ""
        }
        <p class="library-hero-actions">
          ${playWatchButtons(movieWatchHref(movie, "solo"), movieWatchHref(movie, "jam"))}
        </p>
      </div>
    </div>
  `;
}

function libraryTitle(): string {
  const nav = currentRoute;
  if (nav.view === "series") {
    const show = findSeries(nav.seriesId);
    return show ? displayName(show) : "Série";
  }
  if (nav.view === "season") {
    const show = findSeries(nav.seriesId);
    const season = show?.seasons.find((s) => s.number === nav.season);
    return season?.title?.trim() || `Saison ${nav.season}`;
  }
  if (nav.view === "movie") {
    const movie = catalog?.movies.find((m) => m.id === nav.movieId);
    return movie ? displayName(movie) : "Film";
  }
  return "Bibliothèque";
}

function breadcrumbChevron(): string {
  return `<svg class="breadcrumb-chevron" width="14" height="14" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
    <path fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round" d="M9 6l6 6-6 6"/>
  </svg>`;
}

function breadcrumbCrumb(opts: { label: string; target?: string; current?: boolean }): string {
  if (opts.current || !opts.target) {
    return `<span class="breadcrumb-current" aria-current="page">${escapeHtml(opts.label)}</span>`;
  }
  return `<button type="button" class="breadcrumb-link" data-nav="${escapeHtml(opts.target)}">${escapeHtml(opts.label)}</button>`;
}

function renderBreadcrumb(): string {
  const nav = currentRoute;
  const parts: string[] = [
    breadcrumbCrumb({
      label: "Bibliothèque",
      target: nav.view === "library" ? undefined : "library",
      current: nav.view === "library",
    }),
  ];

  if (nav.view === "series" || nav.view === "season") {
    const show = findSeries(nav.seriesId);
    const seriesLabel = show ? displayName(show) : "Série";
    parts.push(breadcrumbChevron());
    parts.push(
      breadcrumbCrumb({
        label: seriesLabel,
        target: nav.view === "season" ? `series:${nav.seriesId}` : undefined,
        current: nav.view === "series",
      }),
    );
  }

  if (nav.view === "season") {
    const show = findSeries(nav.seriesId);
    const season = show?.seasons.find((s) => s.number === nav.season);
    const seasonLabel = season?.title?.trim() || `Saison ${nav.season}`;
    parts.push(breadcrumbChevron());
    parts.push(breadcrumbCrumb({ label: seasonLabel, current: true }));
  }

  if (nav.view === "movie") {
    parts.push(breadcrumbChevron());
    parts.push(breadcrumbCrumb({ label: libraryTitle(), current: true }));
  }

  return `<nav class="breadcrumb" aria-label="Fil d'Ariane">${parts.join("")}</nav>`;
}

function bindBreadcrumbClicks(): void {
  document.querySelectorAll(".breadcrumb-link").forEach((btn) => {
    btn.addEventListener("click", () => {
      const target = (btn as HTMLElement).dataset.nav;
      if (!target) return;
      if (target === "library") {
        goToRoute({ view: "library" });
      } else if (target.startsWith("series:")) {
        goToRoute({ view: "series", seriesId: target.slice("series:".length) });
      }
    });
  });
}

function bindPosterClicks(): void {
  document.querySelectorAll(".poster-card").forEach((btn) => {
    btn.addEventListener("click", () => {
      const el = btn as HTMLElement;
      const kind = el.dataset.kind;
      const id = el.dataset.id;
      if (!kind || !id) return;
      if (kind === "series") goToRoute({ view: "series", seriesId: id });
      else if (kind === "movie") goToRoute({ view: "movie", movieId: id });
      else if (kind === "season") {
        const seriesId = el.dataset.series;
        const season = Number(id);
        if (seriesId && Number.isFinite(season)) {
          goToRoute({ view: "season", seriesId, season });
        }
      }
    });
  });
}

function renderCockpit(): void {
  if (!settings) return;

  const nav = currentRoute;
  let body = "";
  if (nav.view === "library") body = renderLibraryHome();
  else if (nav.view === "series") body = renderSeriesPage(nav.seriesId);
  else if (nav.view === "season") {
    body = renderSeasonPage(nav.seriesId, nav.season);
  } else if (nav.view === "movie") {
    body = renderMoviePage(nav.movieId);
  } else {
    body = renderLibraryHome();
  }

  main.innerHTML = `
    <section class="library" aria-label="Bibliothèque">
      <div class="library-toolbar">
        ${renderBreadcrumb()}
        ${renderLibrarySearch()}
        <div class="library-toolbar-actions">
          <button type="button" class="ghost library-config" id="reopen-wizard">Modifier la configuration</button>
        </div>
      </div>
      <div id="library-body">${body}</div>
    </section>
  `;

  document.getElementById("reopen-wizard")?.addEventListener("click", () => {
    if (!settings) return;
    settings = { ...settings, wizardStep: "options", setupComplete: false };
    currentRoute = { view: "setup", step: "options" };
    void saveAndRender();
  });
  bindLibrarySearch();
  bindBreadcrumbClicks();
  bindPosterClicks();
  bindPrepareClicks();
}

function refreshLibraryBody(): void {
  const host = document.getElementById("library-body");
  if (!host) return;
  const nav = currentRoute;
  if (nav.view === "library") host.innerHTML = renderLibraryHome();
  else if (nav.view === "series") host.innerHTML = renderSeriesPage(nav.seriesId);
  else if (nav.view === "season") {
    host.innerHTML = renderSeasonPage(nav.seriesId, nav.season);
  } else if (nav.view === "movie") {
    host.innerHTML = renderMoviePage(nav.movieId);
  } else {
    host.innerHTML = renderLibraryHome();
  }
  bindPosterClicks();
  bindPrepareClicks();
}

function applyLibrarySearchQuery(value: string, opts?: { focus?: boolean }): void {
  librarySearchQuery = value;
  if (currentRoute.view !== "library") {
    goToRoute({ view: "library" }, { replace: true });
    const next = document.getElementById("library-search") as HTMLInputElement | null;
    if (opts?.focus !== false) {
      next?.focus();
      next?.setSelectionRange(librarySearchQuery.length, librarySearchQuery.length);
    }
    return;
  }
  const input = document.getElementById("library-search") as HTMLInputElement | null;
  if (input && input.value !== value) input.value = value;
  refreshLibraryBody();
  document
    .querySelector(".library-search")
    ?.classList.toggle("has-query", librarySearchQuery.trim().length > 0);
  if (opts?.focus) input?.focus();
}

function bindLibrarySearch(): void {
  const input = document.getElementById("library-search") as HTMLInputElement | null;
  if (!input) return;
  input.addEventListener("input", () => {
    applyLibrarySearchQuery(input.value);
  });
  document.getElementById("library-search-clear")?.addEventListener("click", () => {
    applyLibrarySearchQuery("", { focus: true });
  });
}

function bindPrepareClicks(): void {
  document.getElementById("prepare-season")?.addEventListener("click", () => {
    void runPrepareSeason();
  });
  document.querySelectorAll("[data-prepare-path]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const path = (btn as HTMLElement).dataset.preparePath;
      if (!path) return;
      void runPrepareEpisode(path);
    });
  });
  document.querySelectorAll("[data-cancel-prepare]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const path = (btn as HTMLElement).dataset.cancelPrepare;
      if (!path) return;
      void runCancelPrepare(path);
    });
  });
  document.querySelectorAll("[data-open-watch]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const href = (btn as HTMLElement).dataset.openWatch;
      if (!href) return;
      void openWatchFromHref(href, btn as HTMLElement);
    });
  });
}

function pendingEpisodePaths(): string[] {
  if (currentRoute.view !== "season") return [];
  const nav = currentRoute;
  const show = findSeries(nav.seriesId);
  const season = show?.seasons.find((s) => s.number === nav.season);
  if (!season) return [];
  return season.episodes
    .filter((ep) => {
      const status = ep.prep?.status ?? "missing";
      return (
        status !== "ready" &&
        status !== "partial" &&
        status !== "processing" &&
        status !== "queued"
      );
    })
    .map((ep) => ep.path);
}

async function runPrepareSeason(): Promise<void> {
  const paths = pendingEpisodePaths();
  if (paths.length === 0) return;
  paths.forEach((path, index) => {
    applyEpisodePrep(path, {
      status: index === 0 ? "processing" : "queued",
      video: false,
      subsEn: false,
      subsFr: false,
      enSource: "missing",
      frSource: "missing",
      message: index === 0 ? "Préparation en cours…" : "En file",
    });
  });
  renderCockpit();
  try {
    await prepareEpisodes(paths);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    paths.forEach((path) => {
      const episode = findEpisodeByPath(path);
      if (episode?.prep?.status === "ready") return;
      applyEpisodePrep(path, {
        status: "error",
        video: false,
        subsEn: false,
        subsFr: false,
        enSource: "missing",
        frSource: "missing",
        message,
      });
    });
    renderCockpit();
  }
}

function findEpisodeByPath(path: string): CatalogEpisode | undefined {
  if (!catalog) return undefined;
  for (const series of catalog.series) {
    for (const season of series.seasons) {
      for (const episode of season.episodes) {
        if (episode.path === path) return episode;
      }
    }
  }
  return undefined;
}

function updatePrepProgressDom(path: string, prep: EpisodePrep): boolean {
  let bar: HTMLElement | null = null;
  for (const el of document.querySelectorAll<HTMLElement>("[data-prep-progress]")) {
    if (el.getAttribute("data-prep-progress") === path) {
      bar = el;
      break;
    }
  }
  if (!bar) return false;
  const pct = Math.max(0, Math.min(100, prep.progress ?? 0));
  const label = prep.message?.trim() || "Préparation…";
  const canceling = /annul/i.test(label);
  bar.classList.toggle("is-canceling", canceling);
  bar.setAttribute("aria-valuenow", String(pct));
  bar.setAttribute(
    "aria-label",
    canceling ? "Annulation en cours" : `Préparation ${pct} %, cliquer pour annuler`,
  );
  bar.title = canceling ? "Annulation…" : "Survolez puis cliquez pour annuler";
  const fill = bar.querySelector<HTMLElement>(".episode-prep-bar-fill");
  const text = bar.querySelector<HTMLElement>(".episode-prep-bar-label");
  if (fill) fill.style.width = `${pct}%`;
  if (text) text.textContent = canceling ? "…" : `${pct}%`;
  return true;
}

async function runCancelPrepare(path: string): Promise<void> {
  const current = findEpisodeByPath(path)?.prep;
  applyEpisodePrep(path, {
    status: "processing",
    video: current?.video ?? false,
    subsEn: current?.subsEn ?? false,
    subsFr: current?.subsFr ?? false,
    enSource: current?.enSource ?? "missing",
    frSource: current?.frSource ?? "missing",
    progress: current?.progress ?? 0,
    message: "Annulation…",
  });
  if (!updatePrepProgressDom(path, {
    status: "processing",
    video: current?.video ?? false,
    subsEn: current?.subsEn ?? false,
    subsFr: current?.subsFr ?? false,
    enSource: current?.enSource ?? "missing",
    frSource: current?.frSource ?? "missing",
    progress: current?.progress ?? 0,
    message: "Annulation…",
  })) {
    renderCockpit();
  }
  try {
    const prep = await cancelPrepare(path);
    applyEpisodePrep(path, prep);
  } catch (err) {
    applyEpisodePrep(path, {
      status: "error",
      video: false,
      subsEn: false,
      subsFr: false,
      enSource: "missing",
      frSource: "missing",
      message: err instanceof Error ? err.message : String(err),
    });
  }
  renderCockpit();
  promoteNextQueued();
}

async function runPrepareEpisode(path: string): Promise<void> {
  applyEpisodePrep(path, {
    status: "processing",
    video: false,
    subsEn: false,
    subsFr: false,
    enSource: "missing",
    frSource: "missing",
    progress: 0,
    message: "Préparation en cours…",
  });
  renderCockpit();
  try {
    const prep = await prepareEpisode(path);
    applyEpisodePrep(path, prep);
  } catch (err) {
    applyEpisodePrep(path, {
      status: "error",
      video: false,
      subsEn: false,
      subsFr: false,
      enSource: "missing",
      frSource: "missing",
      message: err instanceof Error ? err.message : String(err),
    });
  }
  renderCockpit();
}

function renderCurrentView(): void {
  if (settings?.setupComplete) renderCockpit();
  else renderWizard();
}

async function ensureCatalog(force = false): Promise<void> {
  if (catalogBusy) {
    // A force scan was requested while busy: defer it until the current scan completes.
    if (force) catalogForcePending = true;
    return;
  }
  if (mediaRoots.length === 0) {
    catalog = { scannedAt: "", series: [], movies: [] };
    catalogError = "";
    return;
  }
  if (!force) {
    if (!catalog) await refreshCatalogIntoUi();
    if (catalog && (catalog.scannedAt || catalogItemCount(catalog) > 0)) {
      return;
    }
  }
  catalogBusy = true;
  catalogForcePending = false;
  catalogError = "";
  const gen = ++catalogGeneration;
  renderCurrentView();
  try {
    catalog = await scanCatalog();
    // Re-read via best effort in case the invoke payload was incomplete.
    if (gen === catalogGeneration) {
      await refreshCatalogIntoUi();
    }
  } catch (err) {
    if (gen === catalogGeneration) {
      catalogError = err instanceof Error ? err.message : String(err);
      await refreshCatalogIntoUi();
    }
  } finally {
    catalogBusy = false;
    if (gen === catalogGeneration) {
      renderCurrentView();
    }
    // If a force scan was requested while busy, run it now.
    if (catalogForcePending) {
      catalogForcePending = false;
      void ensureCatalog(true);
    }
  }
}

function showMediaHint(message: string): void {
  const hint = document.getElementById("media-hint");
  if (!hint) return;
  hint.textContent = message;
  hint.hidden = false;
}

async function refreshMediaRoots(): Promise<void> {
  mediaRoots = await loadMediaRoots();
}

async function refreshServerStatus(): Promise<void> {
  serverStatus = await loadServerStatus();
  if (serverStatus.running || serverStatus.error) return;
  await new Promise((resolve) => window.setTimeout(resolve, 150));
  serverStatus = await loadServerStatus();
}

function catalogPort(): number {
  return serverStatus?.port || settings?.serverPort || 8787;
}

/** Prefer the LAN API when it has more items (avoids stale / truncated IPC snapshots). */
async function loadCatalogBestEffort(): Promise<CatalogSnapshot> {
  let invoked: CatalogSnapshot | null = null;
  try {
    const raw = await loadCatalog();
    invoked = isCatalogSnapshot(raw) ? raw : null;
  } catch {
    invoked = null;
  }

  let httpSnap: CatalogSnapshot | null = null;
  try {
    const res = await fetch(`http://127.0.0.1:${catalogPort()}/api/library`);
    if (res.ok) {
      const raw: unknown = await res.json();
      // Ignore foreign servers on the same port (e.g. POC on :8787).
      httpSnap = isCatalogSnapshot(raw) ? raw : null;
    }
  } catch {
    httpSnap = null;
  }

  if (invoked && httpSnap) {
    return catalogItemCount(httpSnap) > catalogItemCount(invoked)
      ? httpSnap
      : invoked;
  }
  if (httpSnap) return httpSnap;
  if (invoked) return invoked;
  throw new Error("Impossible de charger le catalogue");
}

async function refreshCatalogIntoUi(): Promise<void> {
  try {
    catalog = await loadCatalogBestEffort();
    catalogError = "";
  } catch (err) {
    catalogError = err instanceof Error ? err.message : String(err);
  }
}

async function saveAndRender(opts: { replace?: boolean } = {}): Promise<void> {
  if (!settings) return;
  await persistSettings(settings);
  const replace = opts.replace ?? true;
  if (settings.setupComplete) {
    if (currentRoute.view === "setup") {
      currentRoute = { view: "library" };
    }
    syncHistory(currentRoute, replace);
    await renderCockpitAsync();
  } else {
    currentRoute = { view: "setup", step: settings.wizardStep };
    syncHistory(currentRoute, replace);
    await renderWizardAsync();
  }
}

async function renderCockpitAsync(): Promise<void> {
  networkInfo = await loadNetworkInfo();
  await refreshServerStatus();
  if (settings && serverStatus?.running && settings.serverPort !== serverStatus.port) {
    settings = { ...settings, serverPort: serverStatus.port };
  }
  await refreshCatalogIntoUi();
  renderCockpit();
  void refreshStatusBar();
  const empty =
    !catalog || (!catalog.scannedAt && catalogItemCount(catalog) === 0);
  const legacyIds = (catalog?.series ?? []).some(
    (s) => !s.id.startsWith("local:") && !s.id.startsWith("tmdb:"),
  );
  const force = pendingCatalogScan || empty || legacyIds;
  pendingCatalogScan = false;
  await ensureCatalog(force);
  // Second pass once the HTTP server is definitely up (scan may have finished mid-boot).
  await refreshCatalogIntoUi();
  if (settings?.setupComplete && currentRoute.view !== "setup") {
    renderCockpit();
  }
  void refreshStatusBar();
}

async function renderWizardAsync(): Promise<void> {
  if (!settings) return;
  if (settings.wizardStep === "network") {
    networkInfo = await loadNetworkInfo();
  }
  renderWizard();
  void refreshStatusBar();
}

async function onPickMedia(): Promise<void> {
  readOptionsFromDom();
  const picked = await pickMediaDirectory();
  if (!picked) return;
  try {
    await addMediaRoot(picked);
    await refreshMediaRoots();
    pendingCatalogScan = true;
    if (settings) renderWizard();
  } catch (err) {
    showMediaHint(err instanceof Error ? err.message : String(err));
  }
}

async function onRemoveMedia(id: number): Promise<void> {
  readOptionsFromDom();
  try {
    await removeMediaRoot(id);
    await refreshMediaRoots();
    pendingCatalogScan = true;
    if (settings) renderWizard();
  } catch (err) {
    showMediaHint(err instanceof Error ? err.message : String(err));
  }
}

async function onBack(): Promise<void> {
  if (!settings) return;
  readOptionsFromDom();
  const prev = prevStep(settings.wizardStep);
  if (!prev) return;
  settings = { ...settings, wizardStep: prev };
  currentRoute = { view: "setup", step: prev };
  await saveAndRender({ replace: false });
}

function readOptionsFromDom(): void {
  if (!settings) return;
  if (settings.wizardStep === "media") {
    const check = document.getElementById("library-check") as HTMLSelectElement | null;
    const minutes = Number.parseInt(check?.value ?? "60", 10);
    settings = {
      ...settings,
      libraryCheckMinutes: Number.isFinite(minutes) ? Math.max(0, minutes) : 60,
    };
    return;
  }
  if (settings.wizardStep !== "options") return;
  const skip = document.getElementById("opt-skip-intro") as HTMLInputElement | null;
  const purge = document.getElementById("opt-purge-after-watch") as HTMLInputElement | null;
  const cacheMax = document.getElementById("opt-cache-max") as HTMLSelectElement | null;
  const jamQuiz = document.getElementById("opt-jam-quiz") as HTMLInputElement | null;
  const jamQuizInterval = document.getElementById(
    "opt-jam-quiz-interval",
  ) as HTMLSelectElement | null;
  const jamDisplaySubEn = document.getElementById(
    "opt-jam-display-sub-en",
  ) as HTMLInputElement | null;
  const jamDisplaySubFr = document.getElementById(
    "opt-jam-display-sub-fr",
  ) as HTMLInputElement | null;
  const tmdb = document.getElementById("tmdb-key") as HTMLInputElement | null;
  const parsedGb = Number.parseInt(cacheMax?.value ?? "20", 10);
  const parsedInterval = Number.parseInt(jamQuizInterval?.value ?? "60", 10);

  // TMDB key: if the input is empty and there was a saved key, keep it unless the user
  // explicitly interacted with the field (dataset attribute tracks original state).
  let nextTmdbKey = settings.tmdbApiKey;
  if (tmdb) {
    const inputValue = tmdb.value.trim();
    const hadSavedKey = tmdb.dataset.hasSavedKey === "true";
    if (inputValue) {
      // User typed a new key
      nextTmdbKey = inputValue;
    } else if (hadSavedKey && tmdb.dataset.userCleared === "true") {
      // User explicitly cleared the key
      nextTmdbKey = "";
    }
    // Otherwise keep the existing key
  }

  settings = {
    ...settings,
    skipIntro: skip?.checked ?? settings.skipIntro,
    purgeCacheAfterWatch: purge?.checked ?? settings.purgeCacheAfterWatch,
    cacheMaxGb: Number.isFinite(parsedGb) ? Math.max(0, parsedGb) : 20,
    jamQuizMode: jamQuiz?.checked ?? settings.jamQuizMode,
    jamQuizIntervalSeconds: normalizeQuizInterval(parsedInterval),
    jamDisplaySubEn: jamDisplaySubEn?.checked ?? settings.jamDisplaySubEn,
    jamDisplaySubFr: jamDisplaySubFr?.checked ?? settings.jamDisplaySubFr,
    tmdbApiKey: nextTmdbKey,
  };
}

async function readNetworkFromDom(): Promise<boolean> {
  if (!settings || settings.wizardStep !== "network") return true;
  const portInput = document.getElementById("network-port") as HTMLInputElement | null;
  if (!portInput) return true;
  const port = Number.parseInt(portInput.value, 10);
  if (!Number.isFinite(port) || port < 1024 || port > 65535) {
    portInput.focus();
    return false;
  }
  settings = { ...settings, serverPort: port };
  return true;
}

async function onNext(): Promise<void> {
  if (!settings) return;
  readOptionsFromDom();
  if (!(await readNetworkFromDom())) return;
  const n = nextStep(settings.wizardStep);
  if (!n) {
    currentRoute = { view: "library" };
    // pendingCatalogScan stays true only if media roots changed in the wizard.
    settings = { ...settings, setupComplete: true };
    await saveAndRender({ replace: false });
    return;
  }
  settings = { ...settings, wizardStep: n };
  currentRoute = { view: "setup", step: n };
  await saveAndRender({ replace: false });
}

function locationLooksLikeCatalog(): boolean {
  return locationLooksLikeCatalogPath(window.location.pathname);
}

function applyRouteFromHistory(): void {
  const fromUrl = routeFromLocation() ?? routeFromSettings();
  currentRoute = fromUrl;
  if (fromUrl.view === "setup") {
    if (settings) {
      settings = { ...settings, wizardStep: fromUrl.step, setupComplete: false };
      void persistSettings(settings);
    }
    void renderWizardAsync();
    return;
  }
  if (settings && !settings.setupComplete) {
    settings = { ...settings, setupComplete: true };
    void persistSettings(settings);
  }
  renderCockpit();
}

async function boot(): Promise<void> {
  settings = normalizeSettings(await loadSettings());
  await refreshMediaRoots();
  void listen("library-updated", () => {
    void (async () => {
      await refreshCatalogIntoUi();
      if (settings?.setupComplete && currentRoute.view !== "setup") renderCockpit();
    })();
  });
  void listen<{ path: string; prep: EpisodePrep }>("episode-prep-updated", (event) => {
    const prev = findEpisodeByPath(event.payload.path)?.prep;
    applyEpisodePrep(event.payload.path, event.payload.prep);
    const next = event.payload.prep;
    const progressOnly =
      prev?.status === "processing" &&
      next.status === "processing" &&
      updatePrepProgressDom(event.payload.path, next);
    if (progressOnly) return;
    promoteNextQueued();
    if (settings?.setupComplete && currentRoute.view === "season") renderCockpit();
  });

  if (settings.setupComplete || locationLooksLikeCatalog()) {
    await refreshCatalogIntoUi();
  }

  const fromUrl = routeFromLocation();
  if (fromUrl) {
    currentRoute = fromUrl;
    if (fromUrl.view === "setup") {
      settings = { ...settings, wizardStep: fromUrl.step, setupComplete: false };
      await persistSettings(settings);
      syncHistory(currentRoute, true);
      await renderWizardAsync();
    } else {
      if (!settings.setupComplete) {
        settings = { ...settings, setupComplete: true };
        await persistSettings(settings);
      }
      syncHistory(currentRoute, true);
      await renderCockpitAsync();
    }
  } else {
    currentRoute = routeFromSettings();
    syncHistory(currentRoute, true);
    if (currentRoute.view === "setup") await renderWizardAsync();
    else await renderCockpitAsync();
  }

  window.addEventListener("popstate", () => {
    applyRouteFromHistory();
  });

  if (settings.setupComplete) {
    void maybeCheckUpdatesOnBoot().then(() => {
      renderStatusBar();
    });
    startHourlyUpdateChecks(() => {
      renderStatusBar();
    });
  }

  ensureStatusBarLoop();
}

void boot();
