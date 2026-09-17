import { modeHint, modeLabel } from "./modes.js";

const { invoke } = window.__TAURI__.core;

const apiKeyInput = document.getElementById("api-key");
const languageSelect = document.getElementById("language");
const languageHint = document.getElementById("language-hint");
const hotkeySelect = document.getElementById("hotkey");
const microphoneSelect = document.getElementById("microphone");
const microphoneHint = document.getElementById("microphone-hint");
const autostartInput = document.getElementById("autostart");
const statusPill = document.getElementById("status-pill");
const statusText = statusPill.querySelector(".status-text");
const saveButton = document.getElementById("save-config");
const testButton = document.getElementById("test-api-key");
const toast = document.getElementById("toast");
const modeBarValue = document.getElementById("mode-bar-value");
const tipHotkey = document.getElementById("tip-hotkey");

let toastTimer = null;

function setStatus(text, isError = false) {
  statusText.textContent = text;
  // The pill clips long errors, so keep the full text reachable on hover.
  statusPill.title = text;
  statusPill.classList.toggle("error", isError);
}

function showToast(message) {
  const toastText = toast.querySelector(".toast-text");
  toastText.textContent = message;
  toast.classList.add("visible");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toast.classList.remove("visible"), 2200);
}

function renderLanguage(language) {
  languageSelect.value = language;
  languageHint.textContent = modeHint(language);
  modeBarValue.textContent = modeLabel(language);
}

function renderHotkeys() {
  tipHotkey.textContent = hotkeySelect.value;
}

function renderMicrophoneHint() {
  if (microphoneSelect.value === "default") {
    microphoneHint.textContent = "Folgt automatisch dem aktiven Windows-Mikrofon";
  } else {
    microphoneHint.textContent = "Fest ausgewähltes Mikrofon";
  }
}

async function refreshMicrophones(currentSelection = "default") {
  try {
    const devices = await invoke("list_microphones");
    const active = currentSelection || microphoneSelect.value || "default";
    microphoneSelect.innerHTML = "";

    const defaultOption = document.createElement("option");
    defaultOption.value = "default";
    defaultOption.textContent = "Standard (Windows-Standard)";
    microphoneSelect.appendChild(defaultOption);

    let matchFound = active === "default";
    for (const device of devices) {
      const option = document.createElement("option");
      option.value = device;
      option.textContent = device;
      if (device === active) {
        option.selected = true;
        matchFound = true;
      }
      microphoneSelect.appendChild(option);
    }

    if (!matchFound && active && active !== "default") {
      const customOption = document.createElement("option");
      customOption.value = active;
      customOption.textContent = `${active} (getrennt)`;
      customOption.selected = true;
      microphoneSelect.appendChild(customOption);
    } else if (!matchFound) {
      defaultOption.selected = true;
    }

    renderMicrophoneHint();
  } catch (err) {
    console.error("Failed to list microphones:", err);
  }
}

function populateForm(config) {
  apiKeyInput.value = config.api_key ?? "";
  hotkeySelect.value = config.hotkey ?? "Ctrl+Win";
  autostartInput.checked = Boolean(config.autostart);
  renderLanguage(config.language ?? "auto");
  renderHotkeys();
}

async function loadConfig() {
  try {
    const config = await invoke("get_config");
    await refreshMicrophones(config.microphone ?? "default");
    populateForm(config);
    setStatus("Ready");
  } catch (error) {
    setStatus(`Error: ${error}`, true);
  }
}

async function verifyApiKey() {
  const apiKey = apiKeyInput.value.trim();
  if (!apiKey) {
    setStatus("API key missing", true);
    return false;
  }

  setStatus("Testing…");
  try {
    await invoke("test_api_key", { apiKey });
    setStatus("API key valid");
    return true;
  } catch (error) {
    setStatus("API key invalid", true);
    return false;
  }
}

languageSelect.addEventListener("change", () => {
  renderLanguage(languageSelect.value);
});

hotkeySelect.addEventListener("change", renderHotkeys);
microphoneSelect.addEventListener("change", renderMicrophoneHint);

window.addEventListener("focus", () => {
  refreshMicrophones(microphoneSelect.value);
});

saveButton.addEventListener("click", async () => {
  const config = {
    api_key: apiKeyInput.value.trim(),
    language: languageSelect.value,
    hotkey: hotkeySelect.value,
    autostart: autostartInput.checked,
    microphone: microphoneSelect.value,
  };

  try {
    await invoke("save_config_cmd", { config });
    setStatus("Ready");
    showToast("Settings saved");
  } catch (error) {
    setStatus("Save failed", true);
  }
});

testButton.addEventListener("click", async () => {
  const valid = await verifyApiKey();
  if (valid) {
    showToast("API key is valid");
  }
});

document.getElementById("get-api-key").addEventListener("click", () => {
  invoke("open_url", { url: "https://console.groq.com/keys" });
});

window.__TAURI__.event.listen("pipeline-error", (event) => {
  const message = event.payload ?? "Recording failed";
  setStatus(message, true);
  showToast(message);
});

// Keeps the open settings window in sync when the mode is switched by hotkey.
window.__TAURI__.event.listen("language-changed", (event) => {
  renderLanguage(event.payload ?? "auto");
});

// Re-check the saved API key every time the settings window is opened, so the
// status pill always shows the current key state without a manual test click.
window.__TAURI__.event.listen("settings-shown", () => {
  verifyApiKey();
});

loadConfig();
