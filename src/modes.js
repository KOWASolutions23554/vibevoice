// Shared labels for the language mode, used by the settings window and the
// recording overlay so both always say the same thing.

const LABELS = {
  auto: "Auto-Erkennung",
  de: "Deutsch",
  "de-en": "Deutsch → English",
  en: "English",
  uk: "Українська",
  fr: "Français",
  es: "Español",
  it: "Italiano",
};

const BADGES = {
  auto: "AUTO",
  "de-en": "DE→EN",
};

const HINTS = {
  auto: "Erkennt Sprache automatisch",
  de: "Deutsch rein → Deutsch raus",
  "de-en": "Deutsch rein → Englisch raus",
  en: "English in → English out",
};

export function modeLabel(language) {
  return LABELS[language] ?? language.toUpperCase();
}

export function modeBadge(language) {
  return BADGES[language] ?? language.toUpperCase();
}

export function modeHint(language) {
  return HINTS[language] ?? "";
}
