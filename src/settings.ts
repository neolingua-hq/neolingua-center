import { invoke } from "@tauri-apps/api/core";

export type WizardStep = "welcome" | "media" | "options" | "network";

export type AppSettings = {
  wizardStep: WizardStep;
  skipIntro: boolean;
  serverPort: number;
  setupComplete: boolean;
  tmdbApiKey: string;
  /** 0 = off. Otherwise minutes between background folder checks. */
  libraryCheckMinutes: number;
  /** 0 = unlimited. Otherwise remux/subs cache ceiling in GiB. */
  cacheMaxGb: number;
  /** Delete remux/subs cache after a media is watched to the end. */
  purgeCacheAfterWatch: boolean;
  /** Host default: start Jam with quiz mode. */
  jamQuizMode: boolean;
  /** Host default seconds between Jam quiz rounds. */
  jamQuizIntervalSeconds: number;
  /** Host default: English subtitles on the Jam video display. */
  jamDisplaySubEn: boolean;
  /** Host default: French subtitles on the Jam video display. */
  jamDisplaySubFr: boolean;
};

export type MediaRoot = {
  id: number;
  path: string;
  sortOrder: number;
  accessible: boolean;
};

export const STEP_ORDER: WizardStep[] = [
  "welcome",
  "media",
  "options",
  "network",
];

export const STEP_LABELS: Record<WizardStep, string> = {
  welcome: "Bienvenue",
  media: "Médias",
  options: "Options",
  network: "Réseau",
};

export function stepIndex(step: WizardStep): number {
  return STEP_ORDER.indexOf(step);
}

export function nextStep(step: WizardStep): WizardStep | null {
  const i = stepIndex(step);
  return i < STEP_ORDER.length - 1 ? STEP_ORDER[i + 1]! : null;
}

export function prevStep(step: WizardStep): WizardStep | null {
  const i = stepIndex(step);
  return i > 0 ? STEP_ORDER[i - 1]! : null;
}

export async function loadSettings(): Promise<AppSettings> {
  return invoke<AppSettings>("get_app_settings");
}

export async function persistSettings(settings: AppSettings): Promise<void> {
  await invoke("save_app_settings", { settings });
}

export type NetworkInfo = {
  addresses: string[];
  hostName: string | null;
  localhost: string;
};

export async function loadNetworkInfo(): Promise<NetworkInfo> {
  return invoke<NetworkInfo>("get_network_info");
}

export type ServerStatus = {
  running: boolean;
  port: number;
  error: string | null;
};

export async function loadServerStatus(): Promise<ServerStatus> {
  return invoke<ServerStatus>("get_server_status");
}

export type PresenceSession = {
  clientId: string;
  mode: "solo" | "jam";
  title?: string | null;
  path?: string | null;
  playing: boolean;
};

export type PresenceSnapshot = {
  sessions: PresenceSession[];
};

/** Read live viewer sessions from the in-process LAN server registry. */
export async function loadPresence(): Promise<PresenceSnapshot> {
  return invoke<PresenceSnapshot>("get_presence");
}

export type SubTrackSource = "native" | "generated" | "missing";
export type PrepStatus =
  | "missing"
  | "partial"
  | "ready"
  | "queued"
  | "processing"
  | "error";

export type EpisodePrep = {
  status: PrepStatus;
  video: boolean;
  subsEn: boolean;
  subsFr: boolean;
  enSource?: SubTrackSource;
  frSource?: SubTrackSource;
  progress?: number | null;
  message?: string | null;
};

export type CatalogEpisode = {
  id: string;
  season: number;
  episode: number;
  label: string;
  path: string;
  title: string | null;
  posterUrl: string | null;
  synopsis: string | null;
  confidence?: number;
  tmdbChecked?: boolean;
  prep?: EpisodePrep;
};

export type CatalogSeason = {
  number: number;
  title: string | null;
  posterUrl: string | null;
  synopsis: string | null;
  episodes: CatalogEpisode[];
};

export type CatalogSeries = {
  id: string;
  title: string;
  root: string;
  posterUrl: string | null;
  backdropUrl: string | null;
  synopsis: string | null;
  tmdbId: number | null;
  originalLanguage: string | null;
  displayTitle: string | null;
  seasons: CatalogSeason[];
};

export type CatalogMovie = {
  id: string;
  title: string;
  path: string;
  posterUrl: string | null;
  backdropUrl: string | null;
  synopsis: string | null;
  tmdbId: number | null;
  originalLanguage: string | null;
  year: number | null;
  displayTitle: string | null;
};

export type CatalogSnapshot = {
  scannedAt: string;
  series: CatalogSeries[];
  movies: CatalogMovie[];
};

export async function loadCatalog(): Promise<CatalogSnapshot> {
  return invoke<CatalogSnapshot>("get_catalog");
}

export async function scanCatalog(): Promise<CatalogSnapshot> {
  return invoke<CatalogSnapshot>("scan_catalog");
}

export async function prepareEpisode(path: string): Promise<EpisodePrep> {
  return invoke<EpisodePrep>("prepare_episode", { path });
}

export async function prepareEpisodes(paths: string[]): Promise<void> {
  await invoke("prepare_episodes", { paths });
}

export async function cancelPrepare(path: string): Promise<EpisodePrep> {
  return invoke<EpisodePrep>("cancel_prepare", { path });
}

export async function openWatchLauncher(url: string): Promise<void> {
  await invoke("open_watch_launcher", { url });
}

export async function loadMediaRoots(): Promise<MediaRoot[]> {
  return invoke<MediaRoot[]>("list_media_roots");
}

export async function addMediaRoot(path: string): Promise<MediaRoot> {
  return invoke<MediaRoot>("add_media_root", { path });
}

export async function removeMediaRoot(id: number): Promise<void> {
  await invoke("remove_media_root", { id });
}

export async function pickMediaDirectory(): Promise<string | null> {
  const picked = await invoke<string | null>("pick_media_directory");
  return picked;
}
