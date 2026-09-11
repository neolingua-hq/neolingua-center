/**
 * Shared viewer presence client id (Solo + Jam).
 */

export const PRESENCE_CLIENT_KEY = "neolingua.viewer.clientId";
export const PRESENCE_INTERVAL_MS = 4000;

export function viewerClientId() {
  let id = localStorage.getItem(PRESENCE_CLIENT_KEY);
  if (!id) {
    id =
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `viewer-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    localStorage.setItem(PRESENCE_CLIENT_KEY, id);
  }
  return id;
}
