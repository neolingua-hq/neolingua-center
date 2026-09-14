/** Shared DOM/HTML helpers for Center (UI + viewer logic). */

/**
 * Escape HTML special characters to prevent XSS.
 */
export function escapeHtml(text: unknown): string {
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
 */
export function safeMediaUrl(url: string | null | undefined): string | null {
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
 */
export function slugify(text: string): string {
  const slug = text
    .normalize("NFD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "item";
}
