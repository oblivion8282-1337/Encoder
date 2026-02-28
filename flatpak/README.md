# Flatpak Build-Anleitung

## Voraussetzungen

### Tools

```sh
# flatpak-builder (Arch/CachyOS)
sudo pacman -S flatpak-builder

# flatpak-builder-tools (flatpak-cargo-generator + flatpak-pip-generator)
# Option A: aus dem AUR
paru -S flatpak-builder-tools
# Option B: direkt aus dem Repo
pip install flatpak-builder-tools
```

### Runtimes und SDKs installieren

```sh
flatpak install flathub \
  org.kde.Platform//6.9 \
  org.kde.Sdk//6.9 \
  com.riverbankcomputing.PyQt.BaseApp//6.9 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08 \
  org.freedesktop.Platform.ffmpeg-full//24.08
```

---

## Schritt 1: Offline-Quellen generieren

Diese Dateien müssen einmalig erstellt werden und nach jeder Änderung an
`Cargo.lock` oder den Python-Abhängigkeiten erneuert werden.

```sh
# Aus dem Projekt-Stammverzeichnis:
just flatpak-sources

# Oder manuell:
flatpak-cargo-generator backend/Cargo.lock -o flatpak/cargo-sources.json
flatpak-pip-generator hatchling -o flatpak/python3-requirements.json
```

Danach liegen in `flatpak/` bereit:
- `cargo-sources.json` – Rust-Crates für Offline-Build
- `python3-requirements.json` – hatchling und Abhängigkeiten

---

## Schritt 2: Bauen und Installieren

```sh
# Bauen + lokal installieren (empfohlen für Entwicklung)
just flatpak-build

# Nur bauen (kein Install):
just flatpak-build-only

# Manuell:
flatpak-builder --user --install --force-clean \
  flatpak/build-dir \
  flatpak/de.michaelproxy.ProxyGenerator.yml
```

---

## Schritt 3: Starten

```sh
flatpak run de.michaelproxy.ProxyGenerator
# oder:
just flatpak-run
```

---

## Aufräumen

```sh
just flatpak-clean
# entfernt: flatpak/build-dir  flatpak/.flatpak-builder
```

---

## Modulübersicht

| # | Modul | Beschreibung |
|---|-------|-------------|
| 1 | `python3-requirements.json` | hatchling (Build-Backend für pip) |
| 2 | `backend` | Rust-Binary (`proxy-generator-backend`) |
| 3 | `braw-bridge` | C++ Binary + BRAW SDK `.so`-Dateien |
| 4 | `r3d-bridge` | C++ Binary (R3D SDK statisch gelinkt) |
| 5 | `frontend` | Python-Package (PyQt6 kommt aus BaseApp) |
| 6 | `desktop-integration` | Desktop-Eintrag, Icon, AppStream-Metainfo |

## Hinweise

**BRAW SDK-Bibliotheken** werden nach `/app/sdk/Libraries/Linux/` installiert,
damit der im Binary eingebettete RPATH (`$ORIGIN/../sdk/Libraries/Linux`) greift.

**R3D SDK** ist statisch gelinkt (`libR3DSDKPIC.a`) – keine Laufzeit-Abhängigkeiten.

**FFmpeg** wird über die Extension `org.freedesktop.Platform.ffmpeg-full//24.08`
bereitgestellt – nicht im Bundle enthalten.
