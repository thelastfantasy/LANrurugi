/*
 * Reader demo cycle and the E-H / EX palette switch. Page copy is baked into each language's HTML
 * at build time (build.mjs), so nothing here touches text except the demo's status label.
 */

/* ---- reader demo: original → detect → erase → translated, on a loop ---------------------- */

const reader = document.querySelector(".reader");
const toggle = document.querySelector(".rb-translate");
const status = document.querySelector(".rf-status");
const steps = [...document.querySelectorAll(".step")];
const STEP_ON = { original: 0, detect: 2, erase: 3, translated: 4 };
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
let timer;

function setState(state) {
  if (!reader) return;
  reader.dataset.state = state;
  toggle.setAttribute("aria-pressed", String(state !== "original"));
  status.textContent = reader.dataset[`status${state[0].toUpperCase()}${state.slice(1)}`];
  steps.forEach((step, i) => step.classList.toggle("on", i < STEP_ON[state]));
}

/** Plays `sequence` of [state, ms] steps; with `loop`, starts over from CYCLE when it runs out. */
function play(sequence, loop) {
  clearTimeout(timer);
  const [[state, ms], ...rest] = sequence;
  setState(state);
  if (rest.length) timer = setTimeout(() => play(rest, loop), ms);
  else if (loop) timer = setTimeout(() => play(CYCLE, loop), ms);
}

const CYCLE = [
  ["original", 2600],
  ["detect", 1300],
  ["erase", 700],
  ["translated", 4200],
];

let auto = !reduced;

// A click takes over from the auto loop: translate on demand, or flip back to the original.
toggle?.addEventListener("click", () => {
  auto = false;
  if (reader.dataset.state !== "original") play([["original", 0]], false);
  else if (reduced) play([["translated", 0]], false);
  else play(CYCLE.slice(1), false);
});

/* ---- E-H / EX palette switch ------------------------------------------------------------ */

const THEME_KEY = "lanrurugi-home-theme";
const darkQuery = matchMedia("(prefers-color-scheme: dark)");

function syncThemeButtons() {
  const active = document.documentElement.dataset.theme || (darkQuery.matches ? "ex" : "eh");
  for (const button of document.querySelectorAll("[data-theme-choice]")) {
    button.setAttribute("aria-pressed", String(button.dataset.themeChoice === active));
  }
}

for (const button of document.querySelectorAll("[data-theme-choice]")) {
  button.addEventListener("click", () => {
    document.documentElement.dataset.theme = button.dataset.themeChoice;
    try {
      localStorage.setItem(THEME_KEY, button.dataset.themeChoice);
    } catch {
      /* storage can be unavailable (private mode, blocked cookies) */
    }
    syncThemeButtons();
  });
}

darkQuery.addEventListener("change", syncThemeButtons);
syncThemeButtons();

// Only animate while the demo is on screen.
if (reader && auto) {
  new IntersectionObserver(([entry]) => {
    if (!auto) return;
    if (entry.isIntersecting) play(CYCLE, true);
    else clearTimeout(timer);
  }).observe(reader);
}

/* ---- copy the compose text ---------------------------------------------------------------- */

const copyButton = document.querySelector(".copy-btn");
copyButton?.addEventListener("click", async () => {
  const label = copyButton.querySelector("span");
  const original = label.textContent;
  try {
    await navigator.clipboard.writeText(document.getElementById("compose-yaml").textContent);
  } catch {
    // Clipboard API needs a secure context; fall back to selecting the text for a manual copy.
    getSelection().selectAllChildren(document.getElementById("compose-yaml"));
    return;
  }
  copyButton.classList.add("done");
  label.textContent = copyButton.dataset.copied;
  setTimeout(() => {
    copyButton.classList.remove("done");
    label.textContent = original;
  }, 1800);
});
