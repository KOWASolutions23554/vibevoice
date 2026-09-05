import { modeBadge, modeLabel } from "./modes.js";

const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;

const stage = document.getElementById("stage");
const statusText = document.getElementById("status-text");
const modeBadgeEl = document.getElementById("mode-badge");
const lockBadge = document.getElementById("lock-badge");

const MODE_FLASH_MS = 1600;

let errorTimer = null;
let flashTimer = null;
let language = "auto";

// Every state change goes through here so a pending error or mode flash
// (style + hide timer) can never leak into the next recording.
function setStage(...states) {
  clearTimeout(errorTimer);
  clearTimeout(flashTimer);
  stage.classList.remove("transcribing", "locked", "error", "mode-flash");
  stage.classList.add("visible", ...states);
}

function renderMode() {
  modeBadgeEl.textContent = modeBadge(language);
}

function setLocked(locked) {
  stage.classList.toggle("locked", locked);
  lockBadge.hidden = !locked;
  statusText.textContent = locked ? "Locked" : "Prompt";
}

function showRecording(event) {
  setStage();
  setLocked(Boolean(event.payload?.locked));
}

function showLocked() {
  setStage();
  setLocked(true);
}

function showTranscribing() {
  setStage("transcribing");
  lockBadge.hidden = true;
  statusText.textContent = "…";
}

function hideOverlay() {
  clearTimeout(errorTimer);
  clearTimeout(flashTimer);
  stage.classList.remove("visible", "transcribing", "locked", "error", "mode-flash");
  lockBadge.hidden = true;
  statusText.textContent = "Prompt";
}

function showError(message) {
  setStage("error");
  lockBadge.hidden = true;
  statusText.textContent = message;
  errorTimer = setTimeout(hideOverlay, 4200);
}

// Confirmation flash after Ctrl+Alt. The overlay closes itself again so a mode
// switch never leaves a pill hanging on screen.
function showModeFlash() {
  setStage("mode-flash");
  lockBadge.hidden = true;
  statusText.textContent = modeLabel(language);
  flashTimer = setTimeout(() => {
    hideOverlay();
    getCurrentWindow().hide();
  }, MODE_FLASH_MS);
}

listen("recording-start", showRecording);
listen("recording-locked", showLocked);
listen("transcribing", showTranscribing);
listen("overlay-hide", hideOverlay);
listen("pipeline-error", (event) => {
  showError(event.payload ?? "Something went wrong");
});
listen("language-changed", (event) => {
  language = event.payload ?? "auto";
  renderMode();
  // While recording or transcribing the pill already says what is going on,
  // so only the badge changes.
  const recording =
    stage.classList.contains("visible") &&
    !stage.classList.contains("error") &&
    !stage.classList.contains("mode-flash");
  if (!recording) {
    showModeFlash();
  }
});

invoke("get_config")
  .then((config) => {
    language = config.language ?? "auto";
  })
  .catch(() => {})
  .finally(renderMode);
