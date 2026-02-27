# Einfaches Übersetzungssystem für Deutsch / Englisch.
# tr(key) gibt den String in der aktuell gesetzten Sprache zurück.

from __future__ import annotations

from typing import Literal

Lang = Literal["de", "en"]

_current: Lang = "de"


def set_language(lang: Lang) -> None:
    global _current
    _current = lang


def get_language() -> Lang:
    return _current


def tr(key: str) -> str:
    entry = _STRINGS.get(key)
    if entry is None:
        return key
    return entry.get(_current, entry.get("en", key))


_STRINGS: dict[str, dict[str, str]] = {
    # ── Toolbar ────────────────────────────────────────────────────────────
    "toolbar.add_files":  {"de": "Dateien hinzufügen",  "en": "Add Files"},
    "toolbar.add_folder": {"de": "Ordner hinzufügen",   "en": "Add Folder"},
    "toolbar.start_all":  {"de": "Alles starten",       "en": "Start All"},
    "toolbar.clear_done": {"de": "Fertige entfernen",   "en": "Remove Done"},
    "toolbar.clear_all":  {"de": "Queue leeren",        "en": "Clear Queue"},
    # ── Tabellen-Header ────────────────────────────────────────────────────
    "table.filename":  {"de": "Dateiname",   "en": "Filename"},
    "table.mode":      {"de": "Modus",       "en": "Mode"},
    "table.status":    {"de": "Status",      "en": "Status"},
    "table.progress":  {"de": "Fortschritt", "en": "Progress"},
    "table.fps":       {"de": "FPS",         "en": "FPS"},
    "table.speed":     {"de": "Speed",       "en": "Speed"},
    # ── Einstellungen – Gruppen ────────────────────────────────────────────
    "grp.output":   {"de": "Ausgabe",             "en": "Output"},
    "grp.mode":     {"de": "Modus",               "en": "Mode"},
    "grp.proxy":    {"de": "Proxy-Einstellungen", "en": "Proxy Settings"},
    "grp.hw":       {"de": "Hardware-Encoding",   "en": "Hardware Encoding"},
    "grp.parallel": {"de": "Parallele Jobs",      "en": "Parallel Jobs"},
    "grp.lang":     {"de": "Sprache",             "en": "Language"},
    "grp.naming":   {"de": "Ausgabe-Benennung",  "en": "Output Naming"},
    "lbl.suffix":       {"de": "Suffix:",     "en": "Suffix:"},
    "lbl.subfolder":    {"de": "Unterordner:", "en": "Subfolder:"},
    "placeholder.suffix":    {"de": "leer = kein Suffix",      "en": "empty = no suffix"},
    "placeholder.subfolder": {"de": "z.B. proxy (leer = keiner)", "en": "e.g. proxy (empty = none)"},
    # ── Widgets ────────────────────────────────────────────────────────────
    "btn.browse":         {"de": "Wählen",                   "en": "Browse"},
    "placeholder.output": {"de": "Ausgabe-Ordner wählen...", "en": "Choose output folder..."},
    "lbl.resolution":     {"de": "Auflösung:",               "en": "Resolution:"},
    "lbl.codec":          {"de": "Codec:",                   "en": "Codec:"},
    "rb.rewrap":          {"de": "Re-Wrap (Audio umkodieren)", "en": "Re-Wrap (transcode audio)"},
    "rb.proxy":           {"de": "Proxy (Transkodierung)",   "en": "Proxy (transcode)"},
    "tooltip.prores_cpu": {"de": "ProRes wird immer per CPU enkodiert.",
                           "en": "ProRes is always encoded on CPU."},
    # ── Job-Status ─────────────────────────────────────────────────────────
    "status.waiting":   {"de": "Wartend",    "en": "Waiting"},
    "status.running":   {"de": "Läuft",      "en": "Running"},
    "status.done":      {"de": "Fertig",     "en": "Done"},
    "status.error":     {"de": "Fehler",     "en": "Error"},
    "status.cancelled": {"de": "Abgebrochen","en": "Cancelled"},
    # ── Kontextmenü ────────────────────────────────────────────────────────
    "ctx.reset":  {"de": "Zurücksetzen", "en": "Reset"},
    "ctx.cancel": {"de": "Abbrechen",    "en": "Cancel"},
    "ctx.remove": {"de": "Entfernen",    "en": "Remove"},
    # ── Statusleiste ───────────────────────────────────────────────────────
    "statusbar.summary":  {"de": "{total} Jobs | {running} laufen | {done} fertig",
                           "en": "{total} jobs | {running} running | {done} done"},
    "statusbar.started":  {"de": "Jobs gestartet.", "en": "Jobs started."},
    "statusbar.added":    {"de": "{added} von {total} Jobs hinzugefügt.",
                           "en": "{added} of {total} jobs added."},
    # ── Datei-Dialoge ──────────────────────────────────────────────────────
    "fdlg.select_files":  {"de": "Videodateien auswählen", "en": "Select Video Files"},
    "fdlg.video_filter":  {"de": "Videodateien",           "en": "Video files"},
    "fdlg.all_files":     {"de": "Alle Dateien",           "en": "All files"},
    "fdlg.select_folder": {"de": "Ordner auswählen",       "en": "Select Folder"},
    "fdlg.output_folder": {"de": "Ausgabe-Ordner wählen",  "en": "Choose Output Folder"},
    # ── Dialoge / Meldungen ────────────────────────────────────────────────
    "dlg.queue_empty_title": {"de": "Queue leer",   "en": "Queue Empty"},
    "dlg.queue_empty_msg":   {
        "de": "Bitte zuerst Dateien über 'Dateien hinzufügen' hinzufügen.",
        "en": "Please add files first via 'Add Files'.",
    },
    "dlg.no_videos_title": {"de": "Keine Videos",  "en": "No Videos"},
    "dlg.no_videos_msg":   {
        "de": "Keine Videodateien im Ordner gefunden.",
        "en": "No video files found in folder.",
    },
    "dlg.output_title":     {"de": "Ausgabe-Ordner",             "en": "Output Folder"},
    "dlg.output_choose":    {"de": "Bitte einen Ausgabe-Ordner wählen.",
                             "en": "Please choose an output folder."},
    "dlg.output_not_found": {"de": "Ordner existiert nicht:\n", "en": "Folder does not exist:\n"},
    "dlg.output_no_write":  {"de": "Kein Schreibzugriff auf:\n","en": "No write access to:\n"},
    "dlg.error_title":      {"de": "Fehler",     "en": "Error"},
    "dlg.no_jobs_added":    {
        "de": "Keine Jobs konnten hinzugefügt werden.\nPrüfen Sie ob das Backend läuft.",
        "en": "No jobs could be added.\nCheck if the backend is running.",
    },
    "dlg.job_error_title": {"de": "Job-Fehler", "en": "Job Error"},
}
