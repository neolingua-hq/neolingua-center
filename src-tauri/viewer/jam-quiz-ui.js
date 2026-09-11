/**
 * Pure quiz sentence HTML for the companion Jam UI.
 */

/**
 * @param {Array<{ type: string, value?: string, id?: string }>} segments
 * @param {Record<string, string>} selections
 * @param {boolean} locked
 * @param {(text: unknown) => string} escapeHtml
 */
export function renderQuizSentenceHtml(segments, selections, locked, escapeHtml) {
  return (segments || [])
    .map((seg) => {
      if (seg.type === "text") return escapeHtml(seg.value);
      const filled = selections[seg.id];
      if (filled) {
        const removable = !locked ? ` role="button" tabindex="0" title="Retirer"` : "";
        return `<span class="jam-quiz-gap is-filled${locked ? "" : " is-removable"}" data-gap="${escapeHtml(seg.id)}"${removable}>${escapeHtml(filled)}</span>`;
      }
      return `<span class="jam-quiz-gap" data-gap="${escapeHtml(seg.id)}">____</span>`;
    })
    .join("");
}
