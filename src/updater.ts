import { invoke } from "@tauri-apps/api/core";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  CHECK_KEY,
  DISMISS_KEY,
  SCHEDULE_KEY,
  escapeHtml,
  formatBytes,
  friendlyCheckError,
  friendlyUpdateError,
  parseLastCheckAt,
  shouldAutoInstallOnBoot,
  shouldClearStaleSchedule,
  shouldShowAvailableBanner,
  shouldThrottleAutoCheck,
  HOUR_MS,
} from "./updateLogic";

export type UpdateProgress = {
  downloaded: number;
  contentLength: number | null;
};

let pendingUpdate: Update | null = null;
let installing = false;
/** Latest known newer version (kept after dismiss so the status bar can still show an icon). */
let availableUpdateVersion: string | null = null;
const updateListeners = new Set<() => void>();

function notifyUpdateState(): void {
  for (const listener of updateListeners) {
    try {
      listener();
    } catch {
      // Ignore listener errors.
    }
  }
}

/** Subscribe to pending / available update changes (status bar, etc.). */
export function onUpdateStateChange(listener: () => void): () => void {
  updateListeners.add(listener);
  return () => {
    updateListeners.delete(listener);
  };
}

export function getAvailableUpdateVersion(): string | null {
  return availableUpdateVersion;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function lastCheckAt(): number {
  return parseLastCheckAt(localStorage.getItem(CHECK_KEY));
}

function markChecked(): void {
  localStorage.setItem(CHECK_KEY, String(Date.now()));
}

function dismissedVersion(): string | null {
  return localStorage.getItem(DISMISS_KEY);
}

function scheduledVersion(): string | null {
  return localStorage.getItem(SCHEDULE_KEY);
}

function clearSchedule(): void {
  localStorage.removeItem(SCHEDULE_KEY);
}

function clearDismiss(): void {
  localStorage.removeItem(DISMISS_KEY);
}

export function dismissPendingUpdate(): void {
  if (pendingUpdate) {
    localStorage.setItem(DISMISS_KEY, pendingUpdate.version);
  }
  clearSchedule();
  pendingUpdate = null;
  hideBanner();
  notifyUpdateState();
}

/** Remember this version and install it automatically on the next app launch. */
export function scheduleUpdateOnNextLaunch(): void {
  if (!pendingUpdate || installing) return;
  localStorage.setItem(SCHEDULE_KEY, pendingUpdate.version);
  clearDismiss();
  renderUpdateBanner(pendingUpdate, { scheduled: true });
  notifyUpdateState();
}

export function cancelScheduledUpdate(): void {
  clearSchedule();
  if (pendingUpdate) {
    renderUpdateBanner(pendingUpdate);
  } else {
    hideBanner();
  }
  notifyUpdateState();
}

function bannerEl(): HTMLElement | null {
  return document.getElementById("update-banner");
}

function hideBanner(): void {
  const el = bannerEl();
  if (!el) return;
  el.hidden = true;
  el.innerHTML = "";
}

function bindBannerActions(): void {
  document.getElementById("update-later")?.addEventListener("click", () => {
    if (installing) return;
    dismissPendingUpdate();
  });
  document.getElementById("update-next-launch")?.addEventListener("click", () => {
    if (installing) return;
    scheduleUpdateOnNextLaunch();
  });
  document.getElementById("update-cancel-schedule")?.addEventListener("click", () => {
    if (installing) return;
    cancelScheduledUpdate();
  });
  document.getElementById("update-now")?.addEventListener("click", () => {
    if (installing) return;
    clearSchedule();
    void installPendingUpdate();
  });
  document.getElementById("update-retry-check")?.addEventListener("click", () => {
    if (installing) return;
    void maybeCheckUpdatesOnBoot();
  });
}

/** Banner when a scheduled install cannot be verified yet (no Update handle). */
function renderScheduledCheckError(version: string, message: string): void {
  const el = bannerEl();
  if (!el) return;
  el.hidden = false;
  el.innerHTML = `
    <div class="update-banner-inner">
      <div class="update-banner-text">
        <p class="update-banner-title">Mise à jour ${escapeHtml(version)} planifiée</p>
        <p class="update-banner-status update-banner-status--error">${escapeHtml(message)}</p>
      </div>
      <div class="update-banner-actions">
        <button type="button" class="ghost" id="update-cancel-schedule">Annuler</button>
        <button type="button" class="primary" id="update-retry-check">Réessayer</button>
      </div>
    </div>
  `;
  bindBannerActions();
  notifyUpdateState();
}

export function renderUpdateBanner(
  update: Update,
  opts?: { progress?: UpdateProgress; error?: string; scheduled?: boolean },
): void {
  const el = bannerEl();
  if (!el) return;
  const notes = (update.body || "").trim();
  const progress = opts?.progress;
  const error = opts?.error;
  const scheduled =
    opts?.scheduled === true || scheduledVersion() === update.version;

  let statusHtml = "";
  if (error) {
    statusHtml = `<p class="update-banner-status update-banner-status--error">${escapeHtml(error)}</p>`;
  } else if (progress) {
    const total =
      progress.contentLength && progress.contentLength > 0
        ? formatBytes(progress.contentLength)
        : null;
    const done = formatBytes(progress.downloaded);
    statusHtml = `<p class="update-banner-status">Téléchargement… ${done}${
      total ? ` / ${total}` : ""
    }</p>`;
  } else if (scheduled && !installing) {
    statusHtml = `<p class="update-banner-status">Prévue au prochain démarrage.</p>`;
  }

  const title = installing
    ? `Installation de la mise à jour ${escapeHtml(update.version)}…`
    : scheduled
      ? `Mise à jour ${escapeHtml(update.version)} planifiée`
      : `Mise à jour ${escapeHtml(update.version)} disponible`;

  let actionsHtml = "";
  if (installing) {
    actionsHtml = `
      <button type="button" class="ghost" disabled>Plus tard</button>
      <button type="button" class="primary" disabled>Installation…</button>
    `;
  } else if (scheduled) {
    actionsHtml = `
      <button type="button" class="ghost" id="update-cancel-schedule">Annuler</button>
      <button type="button" class="primary" id="update-now">Mettre à jour maintenant</button>
    `;
  } else {
    actionsHtml = `
      <button type="button" class="ghost" id="update-later">Plus tard</button>
      <button type="button" class="ghost" id="update-next-launch">Au prochain démarrage</button>
      <button type="button" class="primary" id="update-now">Mettre à jour</button>
    `;
  }

  const showNotes = Boolean(notes) && !scheduled && !installing && !error && !progress;
  const showDefaultNote = !notes && !scheduled && !installing && !error && !progress;

  el.hidden = false;
  el.innerHTML = `
    <div class="update-banner-inner">
      <div class="update-banner-text">
        <p class="update-banner-title">${title}</p>
        ${showNotes ? `<p class="update-banner-notes">${escapeHtml(notes)}</p>` : ""}
        ${
          showDefaultNote
            ? `<p class="update-banner-notes">Une nouvelle version de Neolingua Center est prête.</p>`
            : ""
        }
        ${statusHtml}
      </div>
      <div class="update-banner-actions">
        ${actionsHtml}
      </div>
    </div>
  `;

  bindBannerActions();
  notifyUpdateState();
}

export type CheckResult =
  | { status: "unavailable" }
  | { status: "upToDate" }
  | { status: "available"; update: Update }
  | { status: "error"; message: string };

/**
 * Check GitHub Releases for a newer signed build.
 * Auto checks are throttled to ~1h unless a next-launch schedule is pending.
 */
export async function checkForAppUpdate(opts?: {
  force?: boolean;
}): Promise<CheckResult> {
  if (!isTauriRuntime()) {
    return { status: "unavailable" };
  }

  const planned = scheduledVersion();

  // In-memory hit: still honour a pending schedule (force re-check path used at boot).
  if (!opts?.force && pendingUpdate && !planned) {
    availableUpdateVersion = pendingUpdate.version;
    return { status: "available", update: pendingUpdate };
  }

  if (
    shouldThrottleAutoCheck({
      force: Boolean(opts?.force),
      hasSchedule: Boolean(planned),
      lastCheckAt: lastCheckAt(),
      now: Date.now(),
    })
  ) {
    return pendingUpdate
      ? { status: "available", update: pendingUpdate }
      : { status: "upToDate" };
  }

  try {
    const update = await check();
    markChecked();
    if (!update) {
      pendingUpdate = null;
      availableUpdateVersion = null;
      clearSchedule();
      hideBanner();
      notifyUpdateState();
      return { status: "upToDate" };
    }

    pendingUpdate = update;
    availableUpdateVersion = update.version;
    notifyUpdateState();

    if (shouldClearStaleSchedule(planned, update.version)) {
      clearSchedule();
    }

    if (scheduledVersion() === update.version) {
      renderUpdateBanner(update, { scheduled: true });
      return { status: "available", update };
    }

    if (
      !shouldShowAvailableBanner({
        force: Boolean(opts?.force),
        dismissedVersion: dismissedVersion(),
        latest: update.version,
      })
    ) {
      return { status: "available", update };
    }

    if (opts?.force && dismissedVersion() === update.version) {
      clearDismiss();
    }

    renderUpdateBanner(update);
    return { status: "available", update };
  } catch (e) {
    return { status: "error", message: friendlyCheckError(e) };
  }
}

export async function installPendingUpdate(): Promise<void> {
  const update = pendingUpdate;
  if (!update || installing) return;
  installing = true;
  renderUpdateBanner(update, {
    progress: { downloaded: 0, contentLength: null },
  });

  let downloaded = 0;
  let contentLength: number | null = null;

  const onEvent = (event: DownloadEvent) => {
    switch (event.event) {
      case "Started":
        contentLength = event.data.contentLength ?? null;
        downloaded = 0;
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        break;
      case "Finished":
        break;
    }
    renderUpdateBanner(update, {
      progress: { downloaded, contentLength },
    });
  };

  try {
    await update.downloadAndInstall(onEvent);
    clearSchedule();
    clearDismiss();
    try {
      await invoke("stop_lan_server");
    } catch {
      // Best effort: relaunch anyway if stop fails.
    }
    await relaunch();
  } catch (e) {
    installing = false;
    const stillScheduled = scheduledVersion() === update.version;
    renderUpdateBanner(update, {
      error: friendlyUpdateError(e),
      scheduled: stillScheduled,
    });
  }
}

/**
 * Background check after setup.
 * If the user chose "Au prochain démarrage", install automatically when still available.
 */
export async function maybeCheckUpdatesOnBoot(): Promise<void> {
  const planned = scheduledVersion();
  const result = await checkForAppUpdate({ force: Boolean(planned) });

  if (result.status === "error") {
    if (planned) {
      renderScheduledCheckError(planned, result.message);
    }
    return;
  }

  if (result.status !== "available") return;

  if (shouldAutoInstallOnBoot(planned, result.update.version)) {
    pendingUpdate = result.update;
    await installPendingUpdate();
    return;
  }

  if (dismissedVersion() !== result.update.version) {
    renderUpdateBanner(result.update);
  }
}

let hourlyUpdateTimer: ReturnType<typeof setInterval> | null = null;

/** Periodic background checks (every hour) while Center stays open. */
export function startHourlyUpdateChecks(onDone?: () => void): void {
  if (hourlyUpdateTimer) return;
  hourlyUpdateTimer = window.setInterval(() => {
    void checkForAppUpdate({ force: false }).then(() => {
      onDone?.();
    });
  }, HOUR_MS);
}
