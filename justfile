# Proxy Generator – Build-Orchestrierung
# Baut Backend (Rust) und Frontend (Python) in einem Schritt.

default: build

python := "python"

# Backend (Rust/Cargo) bauen
build-backend:
    cargo build --manifest-path backend/Cargo.toml --release

# Frontend braucht kein Packaging – PYTHONPATH reicht
install-frontend:
    @echo "Frontend wird via PYTHONPATH eingebunden (kein Install nötig)"

# Alles bauen
build: build-backend
    @echo "Build fertig. Starte mit: just run"

# Backend starten (für Tests)
run-backend:
    cargo run --manifest-path backend/Cargo.toml --release

# Frontend starten (venv + PYTHONPATH)
run:
    PYTHONPATH=frontend/src \
    PROXY_GENERATOR_BACKEND=backend/target/release/proxy-generator-backend \
    {{python}} -m proxy_generator

# Aufräumen
clean:
    cargo clean --manifest-path backend/Cargo.toml

# ── Flatpak ──────────────────────────────────────────────────────────────────

# Offline-Quellen für Flatpak generieren (einmalig, nach Cargo.lock-Änderungen)
# Voraussetzung: flatpak-cargo-generator und flatpak-pip-generator im PATH
flatpak-sources:
    flatpak-cargo-generator backend/Cargo.lock -o flatpak/cargo-sources.json
    flatpak-pip-generator hatchling -o flatpak/python3-requirements.json

# Flatpak lokal bauen und installieren
flatpak-build:
    flatpak-builder --user --install --force-clean flatpak/build-dir flatpak/de.michaelproxy.ProxyGenerator.yml

# Nur bauen, nicht installieren (für CI / Inspektion)
flatpak-build-only:
    flatpak-builder --force-clean flatpak/build-dir flatpak/de.michaelproxy.ProxyGenerator.yml

# Flatpak starten
flatpak-run:
    flatpak run de.michaelproxy.ProxyGenerator

# Flatpak-Buildartefakte aufräumen
flatpak-clean:
    rm -rf flatpak/build-dir flatpak/.flatpak-builder

# ── AppImage ──────────────────────────────────────────────────────────────────

# AppImage bauen (Voraussetzung: sudo pacman -S cmake patchelf librsvg)
appimage:
    bash appimage/build.sh

# AppImage starten (nach dem Bauen)
appimage-run:
    ./ProxyGenerator-x86_64.AppImage

# AppImage-Buildartefakte aufräumen
appimage-clean:
    rm -rf appimage/AppDir appimage/.cache ProxyGenerator-x86_64.AppImage
