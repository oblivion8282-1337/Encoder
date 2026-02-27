# Proxy Generator

Erzeugt Proxy-Dateien (niedrig aufgelöste Vorschau-Videos) aus Quellmaterial
mithilfe von FFmpeg.

## Architektur

| Schicht   | Technologie        | Zweck                                    |
|-----------|--------------------|------------------------------------------|
| Frontend  | Python + PyQt6     | GUI, Job-Verwaltung, Fortschrittsanzeige |
| Backend   | Rust + Tokio       | FFmpeg-Prozesssteuerung, IPC-Server      |

Frontend und Backend kommunizieren über JSON-basiertes IPC (Unix Domain Socket).

## Bauen

```sh
just build
```

## Starten

```sh
just run-backend &
just run-frontend
```
