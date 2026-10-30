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
// Animate the switch's pill only from now on: the first sync above must not slide it into place.
requestAnimationFrame(() => {
  for (const group of document.querySelectorAll(".theme-switch")) group.classList.add("ready");
});

// Only animate while the demo is on screen.
if (reader && auto) {
  new IntersectionObserver(([entry]) => {
    if (!auto) return;
    if (entry.isIntersecting) play(CYCLE, true);
    else clearTimeout(timer);
  }).observe(reader);
}

/* ---- copy buttons (each names the code block it copies in data-target) -------------------- */

for (const button of document.querySelectorAll(".copy-btn")) {
  button.addEventListener("click", async () => {
    const label = button.querySelector("span");
    const original = label.textContent;
    const source = document.getElementById(button.dataset.target);
    try {
      await navigator.clipboard.writeText(source.textContent);
    } catch {
      // Clipboard API needs a secure context; fall back to selecting the text for a manual copy.
      getSelection().selectAllChildren(source);
      return;
    }
    button.classList.add("done");
    label.textContent = button.dataset.copied;
    setTimeout(() => {
      button.classList.remove("done");
      label.textContent = original;
    }, 1800);
  });
}

/* ---- host port: the compose block edits it, step 3's URL follows ------------------------- */

const portEditor = document.querySelector(".port-edit");
const portEcho = document.getElementById("start-port-url");
const DEFAULT_PORT = 3000;
const MAX_PORT = 65535;
let port = DEFAULT_PORT;

const portDigits = () => portEditor.textContent.replace(/\D/g, "").slice(0, 5);

/** The field's digits as a usable 1–65535 port, or null while it is empty or out of range. */
function livePort() {
  const digits = portDigits();
  const value = Number(digits);
  return digits && value >= 1 && value <= MAX_PORT ? value : null;
}

function showPort(value = port) {
  if (portEcho) portEcho.textContent = String(value);
}

/** The URL follows every usable keystroke; blur commits it, and an unusable field snaps back. */
function commitPort() {
  const value = livePort();
  if (value) port = value;
  if (portEditor.textContent !== String(port)) portEditor.textContent = String(port);
  showPort();
}

if (portEditor) {
  showPort();
  // Keep the field digits-only without rewriting it while the user is typing (that would move the
  // caret): block non-digit insertions up front, and clean up anything else on blur.
  portEditor.addEventListener("beforeinput", (event) => {
    if (event.inputType === "insertText" && !/^\d+$/.test(event.data ?? "")) event.preventDefault();
  });
  portEditor.addEventListener("input", () => {
    // An unusable draft (empty, 0, past 65535) leaves the URL on the committed port.
    showPort(livePort() ?? port);
  });
  portEditor.addEventListener("blur", commitPort);
  portEditor.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      portEditor.blur();
    } else if (event.key === "Escape") {
      portEditor.textContent = String(port);
      portEditor.blur();
    }
  });
  portEditor.addEventListener("paste", (event) => {
    event.preventDefault();
    const digits = (event.clipboardData?.getData("text") ?? "").replace(/\D/g, "").slice(0, 5);
    if (!digits) return;
    // execCommand keeps the caret and fires `input`; assigning textContent is the fallback.
    if (!document.execCommand("insertText", false, digits)) portEditor.textContent = digits;
  });
}

/* ---- LANraragi deployment → LANrurugi compose (runs in the browser only) ------------------- */

// Only the library is carried over: thumbnails and the database stay LANrurugi's own.
const LEGACY_DIRS = ["content"];

const LEGACY_HOME = "/home/koyomi/lanraragi";
const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

/** LANraragi's container path for its library: the default, or its `LRR_DATA_DIRECTORY` override. */
function legacyTargets(text) {
  const targets = { content: `${LEGACY_HOME}/content` };
  for (const m of text.matchAll(/LRR_DATA_DIRECTORY["']?[ \t]*[=:][ \t]*["']?([^\s"',]+)/g)) {
    const path = m[1].startsWith("/") ? m[1] : `${LEGACY_HOME}/${m[1].replace(/^\.\//, "")}`;
    targets.content = path.replace(/\/+$/, "");
  }
  return targets;
}

/**
 * Host side of the bind mount onto LANraragi's library directory (recognised by the container path
 * it is mounted on), and the host port for 3000.
 */
function parseLanraragi(text) {
  const joined = text.replace(/\\\r?\n/g, " ");
  const targets = legacyTargets(joined);
  const dirOf = (target) => LEGACY_DIRS.find((dir) => targets[dir] === target.replace(/\/+$/, ""));
  // Longest first, so an override such as /data/manga is not read as /data.
  const paths = Object.values(targets).sort((a, b) => b.length - a.length).map(escapeRegExp).join("|");
  const tgt = `(${paths})\\/?(?=[:"'\\s,]|$)`;
  const mounts = {};
  const take = (host, target) => {
    host = host.trim();
    const dir = dirOf(target);
    if (host && dir && !(dir in mounts)) mounts[dir] = host;
  };
  // Quoted "host:target[:opts]" — paths with spaces or drive letters.
  const rest = joined.replace(new RegExp(`(["'])([^"'\\n]*?):${tgt}[^"'\\n]*\\1`, "g"), (_, _quote, host, target) => {
    take(host, target);
    return " ";
  });
  // Unquoted short syntax. `-v` and compose's "- " entries run up to the container path, so a
  // path with spaces copied from a guide still comes through; anything else stops at whitespace.
  const unquoted = [
    `(?:-v|--volume)[=\\s]+(?!-)((?:(?![ \\t]-)[^"'\\n])+?):${tgt}`,
    `^[ \\t]*-[ \\t]+([^"'\\n]+?):${tgt}`,
    `(?:^|[\\s=])([^\\s"'=]+):${tgt}`,
  ];
  for (const pattern of unquoted) {
    for (const m of rest.matchAll(new RegExp(pattern, "gm"))) take(m[1], m[2]);
  }
  // --mount type=bind,source=…,target=…
  for (const m of rest.matchAll(/--mount[=\s]+["']?([^\s"']+)/g)) {
    const fields = {};
    for (const part of m[1].split(",")) {
      const at = part.indexOf("=");
      if (at > 0) fields[part.slice(0, at)] = part.slice(at + 1);
    }
    const host = fields.source ?? fields.src;
    const target = fields.target ?? fields.dst ?? fields.destination;
    if (host && target) take(host, target);
  }
  // Compose long syntax, source and target in either order within one volume entry: the lines
  // between them may not start another entry or name another source/target.
  const value = `["']?([^"'\\n]+?)["']?`;
  const legacy = `["']?${tgt}["']?`;
  const between = `[ \\t]*\\n(?:(?![ \\t]*-[ \\t]|[ \\t]*(?:source|target):)[^\\n]*\\n){0,3}?[ \\t]*`;
  for (const m of rest.matchAll(new RegExp(`source:[ \\t]*${value}${between}target:[ \\t]*${legacy}`, "g"))) {
    take(m[1], m[2]);
  }
  for (const m of rest.matchAll(new RegExp(`target:[ \\t]*${legacy}${between}source:[ \\t]*${value}[ \\t]*$`, "gm"))) {
    take(m[2], m[1]);
  }
  const port =
    joined.match(/(?:-p|--publish)[=\s]+["']?((?:[\d.]+:)?\d+):3000\b/)?.[1] ??
    joined.match(/^\s*-\s*["']?((?:[\d.]+:)?\d+):3000\b/m)?.[1];
  return { mounts, port };
}

const isNamedVolume = (host) => !/^([/.~$]|[A-Za-z]:[\\/])/.test(host);

/** The page's own compose text with the old deployment's directories and port put in. */
function convertDeployment(text, template) {
  const { mounts, port } = parseLanraragi(text);
  if (!mounts.content) return null;
  let out = template;
  for (const dir of LEGACY_DIRS) {
    const host = mounts[dir];
    if (!host) continue;
    const line = new RegExp(`^(\\s*- )[^\\n]*?(:\\/home\\/koyomi\\/lanrurugi\\/${dir})$`, "m");
    const quote = /[\s#"]|^[^\w./~$]/.test(host) || /^[A-Za-z]:/.test(host);
    // Backslashes first: escaping only `"` leaves a trailing `\` in the host free to escape the
    // closing quote and break out of the YAML scalar (CodeQL `js/incomplete-sanitization`).
    const escaped = host.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    out = out.replace(line, (_, lead, tail) =>
      quote ? `${lead}"${escaped}${tail}"` : `${lead}${host}${tail}`,
    );
  }
  // The next port up, so LANrurugi can run next to LANraragi while the move is checked.
  const newPort = port?.replace(/\d+$/, (n) => String(Number(n) < 65535 ? Number(n) + 1 : Number(n) - 1));
  if (newPort) out = out.replace(/^(\s*- )"\d+:3000"$/m, `$1"${newPort}:3000"`);
  const named = LEGACY_DIRS.map((dir) => mounts[dir]).filter((host) => host && isNamedVolume(host));
  if (named.length) {
    out += `\nvolumes:\n${named.map((name) => `  ${name}:\n    external: true`).join("\n")}\n`;
  }
  return { out, mounts, port, newPort, named };
}


const converter = document.querySelector(".converter");
converter?.querySelector(".conv-run").addEventListener("click", () => {
  const text = converter.querySelector("textarea").value;
  const notes = converter.querySelector(".conv-notes");
  const output = converter.querySelector(".conv-out");
  const msg = converter.dataset;
  const result = convertDeployment(text, document.getElementById("compose-yaml").textContent);
  notes.replaceChildren();
  const note = (message, warn) => {
    const item = document.createElement("li");
    item.textContent = message;
    if (warn) item.className = "warn";
    notes.append(item);
  };
  if (!result) {
    output.hidden = true;
    note(msg.notFound, true);
    return;
  }
  document.getElementById("conv-yaml").textContent = result.out;
  output.hidden = false;
  const found = LEGACY_DIRS.filter((dir) => result.mounts[dir]).map((dir) => `${msg[dir]} ${result.mounts[dir]}`);
  if (result.port) found.push(`${msg.port} ${result.port}`);
  note(`${msg.read}${found.join(" · ")}`);
  if (LEGACY_DIRS.some((dir) => /^\.\.?[/\\]/.test(result.mounts[dir] ?? ""))) note(msg.relative);
  if (result.named.length) note(msg.named.replace("{names}", result.named.join(", ")));
  if (result.newPort) {
    const hostPort = (p) => p.replace(/^.*:/, "");
    note(msg.portMoved.replaceAll("{new}", hostPort(result.newPort)).replaceAll("{old}", hostPort(result.port)));
  }
  note(msg.importNote, true);
});

