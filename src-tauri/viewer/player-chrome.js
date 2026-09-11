/**
 * Fullscreen player chrome idle hide (Solo shell or Jam document FS).
 */

export const CHROME_IDLE_MS = 2200;

/**
 * @param {{
 *   getShell: () => HTMLElement | null | undefined,
 *   shouldHide: () => boolean,
 *   idleMs?: number,
 * }} opts
 */
export function createChromeController(opts) {
  let timer = /** @type {ReturnType<typeof setTimeout> | null} */ (null);
  const idleMs = opts.idleMs ?? CHROME_IDLE_MS;

  function clear() {
    if (timer != null) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function schedule() {
    clear();
    const shell = opts.getShell();
    if (!shell || !opts.shouldHide()) {
      shell?.classList.remove("is-chrome-hidden");
      return;
    }
    timer = setTimeout(() => {
      if (!opts.shouldHide()) return;
      opts.getShell()?.classList.add("is-chrome-hidden");
    }, idleMs);
  }

  /** @param {boolean} [keepVisible] */
  function reveal(keepVisible = false) {
    const shell = opts.getShell();
    if (!shell) return;
    shell.classList.remove("is-chrome-hidden");
    clear();
    if (keepVisible || !opts.shouldHide()) return;
    schedule();
  }

  return { schedule, reveal, clear };
}
