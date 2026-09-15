/**
 * Shared DOM/HTML helpers for the viewer (plain JS, browser-side).
 * Keep in sync with src/shared/html.ts in the Center TS codebase.
 */

/**
 * Escape HTML special characters to prevent XSS.
 * @param {unknown} text
 * @returns {string}
 */
export function escapeHtml(text) {
  return String(text ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Validate and sanitize a URL for use in media src attributes.
 * Only allows http:, https:, or relative paths starting with /.
 * Returns null for invalid or potentially dangerous URLs (e.g. javascript:).
 * @param {string | null | undefined} url
 * @returns {string | null}
 */
export function safeMediaUrl(url) {
  if (!url || typeof url !== "string") return null;
  const trimmed = url.trim();
  if (!trimmed) return null;

  // Relative paths starting with /
  if (trimmed.startsWith("/")) return trimmed;

  // Absolute URLs: only http and https
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol === "http:" || parsed.protocol === "https:") {
      return trimmed;
    }
  } catch {
    // Invalid URL format
  }
  return null;
}

/**
 * Convert a title to a URL-friendly slug.
 * @param {string} text
 * @returns {string}
 */
export function slugify(text) {
  const slug = String(text || "")
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "item";
}

/**
 * Get the display name for an item (displayTitle or title).
 * @param {{ title: string, displayTitle?: string | null }} item
 * @returns {string}
 */
export function displayName(item) {
  return (item.displayTitle && item.displayTitle.trim()) || item.title;
}
