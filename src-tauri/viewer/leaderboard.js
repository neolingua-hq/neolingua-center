/**
 * Shared Jam leaderboard HTML helpers (host + companion).
 */

/** @param {number} ms */
export function formatResponseTime(ms) {
  if (!Number.isFinite(ms) || ms < 0) return "-";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) {
    const rounded = seconds < 10 ? seconds.toFixed(1) : String(Math.round(seconds));
    return `${rounded.replace(".", ",")} s`;
  }
  const minutes = Math.floor(seconds / 60);
  const rem = Math.round(seconds % 60);
  return `${minutes} min ${rem} s`;
}

/**
 * @param {string | undefined} reason
 * @param {string | undefined} by
 */
export function leaderboardReasonText(reason, by) {
  if (reason === "video") return "Fin de l'épisode";
  if (by) return `Terminé par ${by}`;
  return "Jam terminé";
}

/**
 * @param {Array<{
 *   rank: number,
 *   peerId?: string,
 *   name: string,
 *   points: number,
 *   correctAnswers?: number,
 *   wrongAnswers?: number,
 *   totalResponseMs?: number,
 * }>} entries
 * @param {{
 *   escapeHtml: (text: unknown) => string,
 *   highlightPeerId?: string | null,
 * }} opts
 */
export function buildLeaderboardHtml(entries, opts) {
  if (!entries.length) {
    return `<li class="jam-leaderboard-empty">Aucun score pour cette partie</li>`;
  }
  const { escapeHtml, highlightPeerId = null } = opts;
  return entries
    .map((entry) => {
      const me = highlightPeerId && entry.peerId === highlightPeerId ? " is-me" : "";
      const pts = `${entry.points} pt${entry.points === 1 ? "" : "s"}`;
      const correct = entry.correctAnswers ?? 0;
      const wrong = entry.wrongAnswers ?? 0;
      const correctLabel = correct === 1 ? "bonne" : "bonnes";
      const wrongLabel = wrong === 1 ? "erreur" : "erreurs";
      return `<li class="jam-leaderboard-row${me}">
              <span class="jam-leaderboard-rank">${entry.rank}</span>
              <span class="jam-leaderboard-name">${escapeHtml(entry.name)}</span>
              <span class="jam-leaderboard-record" title="Bonnes réponses et erreurs">
                <span class="jam-leaderboard-ok">${correct} <span class="jam-leaderboard-unit">${correctLabel}</span></span>
                <span class="jam-leaderboard-sep">·</span>
                <span class="jam-leaderboard-ko">${wrong} <span class="jam-leaderboard-unit">${wrongLabel}</span></span>
              </span>
              <span class="jam-leaderboard-points">${escapeHtml(pts)}</span>
              <span class="jam-leaderboard-time">${escapeHtml(formatResponseTime(entry.totalResponseMs ?? 0))}</span>
            </li>`;
    })
    .join("");
}

/**
 * @param {{
 *   boards: Iterable<HTMLElement>,
 *   kickers: Iterable<HTMLElement>,
 *   entries: Parameters<typeof buildLeaderboardHtml>[0],
 *   reason?: string,
 *   by?: string,
 *   escapeHtml: (text: unknown) => string,
 *   highlightPeerId?: string | null,
 * }} args
 */
export function applyLeaderboard(args) {
  const reasonText = leaderboardReasonText(args.reason, args.by);
  for (const kicker of args.kickers) {
    kicker.textContent = reasonText;
  }
  const html = buildLeaderboardHtml(args.entries, {
    escapeHtml: args.escapeHtml,
    highlightPeerId: args.highlightPeerId,
  });
  for (const board of args.boards) {
    board.innerHTML = html;
  }
}
