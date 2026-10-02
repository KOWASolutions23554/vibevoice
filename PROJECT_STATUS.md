# Projektstatus – Vibe Voice Tool

## Stand: 02.10.2026

**Fertig:**
- Remote-Modus für Parsec (02.10., Version 0.1.11): neuer Schalter „Remote-Modus (Parsec)“
  im Einstellungsfenster (`config.remote_typing`). Ist er an, wird der Text nicht über die
  Zwischenablage eingefügt, sondern als echte Tastendrücke (Scancodes, passend zum
  Tastaturlayout) getippt (`InjectMethod::ScancodeType` in clipboard.rs). AltGr-Zeichen
  (@ € \) gehen als Strg+RECHTS-Alt raus, damit der Strg+Alt-Modusumschalter nicht
  auslöst. Tests 18/18 grün, auf PC und Laptop installiert, über Parsec getestet: funktioniert.
- Bugfix Deutsch→Englisch-Übersetzung (24.09., final): Groq hat ALLE alten Chat-Modelle
  abgeschaltet — zuerst `groq/compound-mini`, dann auch den Zwischenfix
  `llama-3.1-8b-instant` (daher funktionierte der erste Fix nicht). Jetzt aktiv:
  `openai/gpt-oss-20b` (src-tauri/src/transcription.rs, Zeile 136) — mit Fabians
  API-Schlüssel live getestet, Übersetzung funktioniert. Exe neu gebaut, Tests (15/15) grün,
  neue Exe in "C:\Program Files\Vibe Voice Tool" ausgetauscht, App läuft.
- Alte, defekte Windows-Installation (24.09. entfernt; Installer-Cache von Windows war
  beschädigt, LocalPackage fehlte) manuell bereinigt und danach sauber neu installiert.

**In Arbeit:**
- Nichts.

**Nächster konkreter Schritt:**
- Nichts offen. Remote-Modus (v0.1.11) am 02.10. von Fabian über Parsec erfolgreich getestet
  (VibeVoice läuft dabei auf dem LAPTOP, gesteuert wird der PC). Updates für den Laptop
  kommen über GitHub-Releases (Tag `v*` pushen → Workflow baut MSI).

**Entscheidungen:**
- 24.09.2026: Linux-Support analysiert, aber von Fabian bewusst NICHT umgesetzt.
  (Analyse nachzulesen in dieser Datei-Historie / Git. Größte Hürde: Wayland erlaubt
  keine globalen Modifier-Hold-Hotkeys; X11-Port wäre machbar, gewünscht ist er nicht.)

**Offene Fragen:**
- Soll der Remote-Modus später automatisch anspringen, wenn das Parsec-Fenster vorne ist
  (statt per Schalter)? Erst nach dem Test entscheiden.

**Stolpersteine:**
- Windows Installer-Cache war beschädigt (fehlende LocalPackage) — Deinstallation per
  msiexec schlug mit 1603 fehl. Lösung: manuelle Bereinigung (Programmordner + Registry)
  in einer erhöhten PowerShell, danach normale MSI-Installation.
