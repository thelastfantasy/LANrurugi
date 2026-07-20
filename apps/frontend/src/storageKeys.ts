// `localStorage` keys shared across more than one page/component — kept in one place so the two
// (or more) files reading/writing the same key can't drift apart on the string itself.

/** Multi-select batch-operation "handshake" — legacy's own `localStorage.getItem/setItem/
 * removeItem("msmSelection")` key name, verified against `~/LANraragi/public/js/mod/index.js`.
 * Library writes the current multi-select set here before navigating to Batch, which reads it
 * once on mount and clears it immediately after (a one-shot handoff, not a persisted setting). */
export const MSM_SELECTION_KEY = 'msmSelection'
