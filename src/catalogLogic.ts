/** Pure catalog / routing helpers for Center (unit-tested). */

import {
  STEP_ORDER,
  type AppSettings,
  type CatalogSnapshot,
  type EpisodePrep,
  type PresenceSession,
  type WizardStep,
} from "./settings";
import { escapeHtml, safeMediaUrl, slugify } from "./shared/html";

export { escapeHtml, safeMediaUrl, slugify };

export type CenterRoute =
  | { view: "setup"; step: WizardStep }
  | { view: "library" }
  | { view: "series"; seriesId: string }
  | { view: "season"; seriesId: string; season: number }
  | { view: "movie"; movieId: string };

export function folderDisplayName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

export function buildSlugById<T extends { id: string; tmdbId?: number | null }>(
  items: T[],
  getTitle: (item: T) => string,
): Map<string, string> {
  const bases = items.map((item) => ({ item, base: slugify(getTitle(item)) }));
  const counts = new Map<string, number>();
  for (const { base } of bases) {
    counts.set(base, (counts.get(base) || 0) + 1);
  }
  const bySlug = new Set<string>();
  const byId = new Map<string, string>();
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
    bySlug.add(candidate);
    byId.set(item.id, candidate);
  }
  return byId;
}

export function displayName(item: {
  title: string;
  displayTitle?: string | null;
}): string {
  return item.displayTitle?.trim() || item.title;
}

export function normalizeSearchText(text: string): string {
  return text
    .toLowerCase()
    .normalize("NFD")
    .replace(/\p{M}/gu, "");
}

export function itemMatchesLibrarySearch(
  item: { title: string; displayTitle?: string | null },
  query: string,
): boolean {
  const q = normalizeSearchText(query.trim());
  if (!q) return true;
  const candidates = [item.title, item.displayTitle?.trim()].filter(Boolean) as string[];
  return candidates.some((name) => normalizeSearchText(name).includes(q));
}

export function formatPresenceLabel(
  sessions: PresenceSession[],
  serverRunning: boolean,
): string {
  const active = sessions.filter((s) => s.playing);
  if (active.length === 0) {
    if (serverRunning) return "Salon libre";
    return "Aucune lecture";
  }

  const hasJam = active.some((s) => s.mode === "jam");
  const hasSolo = active.some((s) => s.mode !== "jam");
  let modeLabel = "Solo";
  if (hasJam && hasSolo) {
    modeLabel = "Solo + Jam";
  } else if (hasJam) {
    modeLabel = "Jam";
  }

  const titled = active.find((s) => (s.title || "").trim());
  const title = titled?.title?.trim();
  if (title) {
    return `<span class="status-bar-mode">${escapeHtml(modeLabel)}</span> · ${escapeHtml(title)}`;
  }
  return `<span class="status-bar-mode">${escapeHtml(modeLabel)}</span> · Lecture en cours`;
}

export function formatPrepMessage(message: string | null | undefined): string {
  if (!message) return "";
  return message.replace(/\s+/g, " ").trim();
}

/** Short "EN …, FR …" constitution lines (shown via flags instead). */
export function isTrackConstitutionMessage(message: string): boolean {
  return /^EN \S+, FR \S+$/i.test(message.trim());
}

export function trackSourceTooltip(source: string): string {
  if (source === "native") return "Présent dans le fichier";
  if (source === "generated") return "Généré depuis l’audio";
  return "";
}

export function formatTrackConstitution(prep: EpisodePrep | null | undefined): string {
  const parts: string[] = [];
  for (const [code, source] of [
    ["EN", prep?.enSource],
    ["FR", prep?.frSource],
  ] as const) {
    if (source !== "native" && source !== "generated") continue;
    const tip = trackSourceTooltip(source);
    parts.push(`${code} : ${tip}`);
  }
  return parts.join(" · ");
}

export function isCatalogSnapshot(value: unknown): value is CatalogSnapshot {
  if (!value || typeof value !== "object") return false;
  const record = value as Record<string, unknown>;
  return Array.isArray(record.series) && Array.isArray(record.movies);
}

export function catalogItemCount(snap: CatalogSnapshot): number {
  return (snap.series?.length ?? 0) + (snap.movies?.length ?? 0);
}

export function normalizeSettings(raw: AppSettings): AppSettings {
  const step = String(raw.wizardStep);
  const known = STEP_ORDER.includes(raw.wizardStep);
  const setupComplete = raw.setupComplete || step === "done";
  let wizardStep = known ? raw.wizardStep : setupComplete ? "network" : "welcome";
  if (step === "tmdb") wizardStep = "options";
  return {
    ...raw,
    tmdbApiKey: raw.tmdbApiKey ?? "",
    libraryCheckMinutes:
      typeof raw.libraryCheckMinutes === "number" ? raw.libraryCheckMinutes : 60,
    cacheMaxGb: typeof raw.cacheMaxGb === "number" ? raw.cacheMaxGb : 20,
    purgeCacheAfterWatch: Boolean(raw.purgeCacheAfterWatch),
    jamQuizMode: Boolean(raw.jamQuizMode),
    jamQuizIntervalSeconds: normalizeQuizInterval(raw.jamQuizIntervalSeconds),
    jamDisplaySubEn: Boolean(raw.jamDisplaySubEn),
    jamDisplaySubFr: Boolean(raw.jamDisplaySubFr),
    setupComplete,
    wizardStep,
  };
}

const QUIZ_INTERVALS = [30, 60, 120, 300, 900] as const;

export { QUIZ_INTERVALS };

export function normalizeQuizInterval(raw: unknown): number {
  const value = Number(raw);
  if (QUIZ_INTERVALS.some((seconds) => seconds === value)) return value;
  return 60;
}

export function normalizePathname(pathname: string): string {
  let path = pathname.replace(/\/+$/, "") || "/";
  if (path.endsWith("/index.html")) {
    path = path.slice(0, -"/index.html".length) || "/";
  }
  return path;
}

export function pathForRoute(
  route: CenterRoute,
  seriesSlug: (seriesId: string) => string,
  movieSlug: (movieId: string) => string,
): string {
  if (route.view === "setup") return `/setup/${route.step}`;
  if (route.view === "library") return "/library";
  if (route.view === "series") {
    return `/series/${encodeURIComponent(seriesSlug(route.seriesId))}`;
  }
  if (route.view === "season") {
    return `/series/${encodeURIComponent(seriesSlug(route.seriesId))}/s${route.season}`;
  }
  return `/movies/${encodeURIComponent(movieSlug(route.movieId))}`;
}

type SeriesRef = { id: string; seasons: { number: number }[] };
type MovieRef = { id: string };

/** null = use settings / default (/, empty, unknown junk). */
export function routeFromPathname(
  pathname: string,
  findSeries: (ref: string) => SeriesRef | undefined,
  findMovie: (ref: string) => MovieRef | undefined,
): CenterRoute | null {
  const rawPath = normalizePathname(pathname);
  if (rawPath === "/" || rawPath === "") return null;

  const setupMatch = rawPath.match(/^\/setup\/([^/]+)$/);
  if (setupMatch) {
    const step = setupMatch[1] as WizardStep;
    if (STEP_ORDER.includes(step)) return { view: "setup", step };
    return { view: "setup", step: "welcome" };
  }

  if (rawPath === "/library") return { view: "library" };

  const seasonMatch =
    rawPath.match(/^\/series\/([^/]+)\/s(\d+)$/i) ||
    rawPath.match(/^\/series\/([^/]+)\/s\/(\d+)$/i);
  if (seasonMatch) {
    const show = findSeries(seasonMatch[1]!);
    const season = Number(seasonMatch[2]);
    if (!show || !show.seasons.some((s) => s.number === season)) {
      return { view: "library" };
    }
    return { view: "season", seriesId: show.id, season };
  }

  const seriesMatch = rawPath.match(/^\/series\/([^/]+)$/);
  if (seriesMatch) {
    const show = findSeries(seriesMatch[1]!);
    if (!show) return { view: "library" };
    return { view: "series", seriesId: show.id };
  }

  const movieMatch = rawPath.match(/^\/movies\/([^/]+)$/);
  if (movieMatch) {
    const movie = findMovie(movieMatch[1]!);
    if (!movie) return { view: "library" };
    return { view: "movie", movieId: movie.id };
  }

  return null;
}

export function locationLooksLikeCatalog(pathname: string): boolean {
  const path = normalizePathname(pathname);
  return path === "/library" || path.startsWith("/series/") || path.startsWith("/movies/");
}

export function routeFromSettings(settings: AppSettings | null): CenterRoute {
  if (!settings) return { view: "setup", step: "welcome" };
  if (!settings.setupComplete) {
    return { view: "setup", step: settings.wizardStep };
  }
  return { view: "library" };
}
