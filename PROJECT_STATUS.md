# Projektstatus – Vibe Voice Tool

## Stand: 24.09.2026

**Fertig:**
- Bugfix Deutsch→Englisch-Übersetzung (24.09., final): Groq hat ALLE alten Chat-Modelle
  abgeschaltet — zuerst `groq/compound-mini`, dann auch den Zwischenfix
  `llama-3.1-8b-instant` (daher funktionierte der erste Fix nicht). Jetzt aktiv:
  `openai/gpt-oss-20b` (src-tauri/src/transcription.rs, Zeile 136) — mit Fabians
  API-Schlüssel live getestet, Übersetzung funktioniert. Exe neu gebaut, Tests (15/15) grün,
  neue Exe in "C:\Program Files\Vibe Voice Tool" ausgetauscht, App läuft.
- Alte, defekte Windows-Installation (24.09. entfernt; Installer-Cache von Windows war
  beschädigt, LocalPackage fehlte) manuell bereinigt und danach sauber neu installiert.

**In Arbeit:**
- Warten auf Fabians Test der Übersetzung (de-en-Modus, Ctrl+Alt).

**Nächster konkreter Schritt:**
- Nichts offen. Fix ist von Fabian getestet und wird mit diesem Handover committet/pusht.
- Beim nächsten Release: Version auf 0.1.10 erhöhen.

**Entscheidungen:**
- 24.09.2026: Linux-Support analysiert, aber von Fabian bewusst NICHT umgesetzt.
  (Analyse nachzulesen in dieser Datei-Historie / Git. Größte Hürde: Wayland erlaubt
  keine globalen Modifier-Hold-Hotkeys; X11-Port wäre machbar, gewünscht ist er nicht.)

**Offene Fragen:**
- Versionsnummer wurde NICHT erhöht (weiterhin 0.1.9). Für den nächsten Release auf
  0.1.10 erhöhen, damit Updates sauber erkannt werden.

**Stolpersteine:**
- Windows Installer-Cache war beschädigt (fehlende LocalPackage) — Deinstallation per
  msiexec schlug mit 1603 fehl. Lösung: manuelle Bereinigung (Programmordner + Registry)
  in einer erhöhten PowerShell, danach normale MSI-Installation.
