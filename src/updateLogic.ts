/** Storage keys and pure helpers for Center auto-update (unit-tested). */

export const CHECK_KEY = "neolingua.center.updateLastCheck";
export const DISMISS_KEY = "neolingua.center.updateDismissed";
export const SCHEDULE_KEY = "neolingua.center.updateOnNextLaunch";
/** Background GitHub check interval (also used as throttle window). */
export const HOUR_MS = 60 * 60 * 1000;

export function parseLastCheckAt(raw: string | null): number {
  const n = raw ? Number(raw) : 0;
  return Number.isFinite(n) ? n : 0;
}

/** Skip a background GitHub check when one ran recently, unless a schedule is pending. */
export function shouldThrottleAutoCheck(opts: {
  force: boolean;
  hasSchedule: boolean;
  lastCheckAt: number;
  now: number;
}): boolean {
  if (opts.force || opts.hasSchedule) return false;
  return opts.now - opts.lastCheckAt < HOUR_MS;
}

export function shouldAutoInstallOnBoot(
  planned: string | null,
  latest: string,
): boolean {
  return Boolean(planned) && planned === latest;
}

/** Latest release no longer matches what the user scheduled. */
export function shouldClearStaleSchedule(
  planned: string | null,
  latest: string,
): boolean {
  return Boolean(planned) && planned !== latest;
}

/** Show the available-update banner (not dismissed, or a forced manual check). */
export function shouldShowAvailableBanner(opts: {
  force: boolean;
  dismissedVersion: string | null;
  latest: string;
}): boolean {
  if (opts.dismissedVersion !== opts.latest) return true;
  return opts.force;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} o`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} Ko`;
  return `${(n / (1024 * 1024)).toFixed(1)} Mo`;
}

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Map plugin / network failures to short French copy (no stack traces). */
export function friendlyUpdateError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const lower = raw.toLowerCase();
  if (
    lower.includes("network") ||
    lower.includes("request") ||
    lower.includes("connect") ||
    lower.includes("timeout") ||
    lower.includes("dns") ||
    lower.includes("fetch")
  ) {
    return "Connexion impossible. Vérifie ton réseau et réessaie.";
  }
  if (lower.includes("signature") || lower.includes("minisign") || lower.includes("verify")) {
    return "La mise à jour n’a pas pu être vérifiée. Réessaie plus tard.";
  }
  if (
    lower.includes("permission") ||
    lower.includes("access") ||
    lower.includes("denied") ||
    lower.includes("readonly")
  ) {
    return "Installation refusée. Vérifie les droits sur l’application.";
  }
  if (lower.includes("not found") || lower.includes("404")) {
    return "Mise à jour introuvable. Réessaie plus tard.";
  }
  return "Impossible d’installer la mise à jour. Réessaie plus tard.";
}

export function friendlyCheckError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err);
  const lower = raw.toLowerCase();
  if (
    lower.includes("network") ||
    lower.includes("request") ||
    lower.includes("connect") ||
    lower.includes("timeout") ||
    lower.includes("dns") ||
    lower.includes("fetch")
  ) {
    return "Connexion impossible. Vérifie ton réseau et réessaie.";
  }
  return "Impossible de vérifier les mises à jour. Réessaie plus tard.";
}
