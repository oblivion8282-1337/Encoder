#!/usr/bin/env bash
# Baut ein AppImage für den Proxy Generator.
#
# Voraussetzungen (einmalig installieren):
#   sudo pacman -S cmake patchelf librsvg
#
# Externe Tools werden automatisch in appimage/.cache/ heruntergeladen:
#   - appimagetool  (AppImage-Paketierung)
#   - Python 3.12   (python-build-standalone, relocatable)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
APPDIR="$SCRIPT_DIR/AppDir"
CACHE_DIR="$SCRIPT_DIR/.cache"
OUTPUT="$ROOT_DIR/ProxyGenerator-x86_64.AppImage"

# ── Versionen ────────────────────────────────────────────────────────────────
PYTHON_VERSION="3.12.8"
PBS_DATE="20241219"
PBS_ARCH="x86_64_v2"   # läuft auf Haswell+ (2013+), gute Kompatibilität
PBS_NAME="cpython-${PYTHON_VERSION}+${PBS_DATE}-${PBS_ARCH}-unknown-linux-gnu-install_only"
PBS_URL="https://github.com/indygreg/python-build-standalone/releases/download/${PBS_DATE}/${PBS_NAME}.tar.gz"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"

# ── Hilfsfunktionen ──────────────────────────────────────────────────────────
log()  { echo "▶ $*"; }
die()  { echo "✗ $*" >&2; exit 1; }
need() { command -v "$1" &>/dev/null || die "Fehlendes Tool: $1  →  sudo pacman -S $2"; }

# ── Voraussetzungen ──────────────────────────────────────────────────────────
need cargo     "rust"
need cmake     "cmake"
need rsvg-convert "librsvg"

mkdir -p "$CACHE_DIR"

# ── appimagetool herunterladen ───────────────────────────────────────────────
APPIMAGETOOL="$CACHE_DIR/appimagetool"
if [[ ! -x "$APPIMAGETOOL" ]]; then
    log "Lade appimagetool..."
    curl -fsSL "$APPIMAGETOOL_URL" -o "$APPIMAGETOOL"
    chmod +x "$APPIMAGETOOL"
fi

# ── Python-Build-Standalone herunterladen ────────────────────────────────────
PYTHON_TAR="$CACHE_DIR/${PBS_NAME}.tar.gz"
if [[ ! -f "$PYTHON_TAR" ]]; then
    log "Lade Python $PYTHON_VERSION (python-build-standalone)..."
    curl -fsSL "$PBS_URL" -o "$PYTHON_TAR"
fi

# ── Rust-Backend bauen ───────────────────────────────────────────────────────
log "Baue Rust-Backend..."
cargo build --manifest-path "$ROOT_DIR/backend/Cargo.toml" --release

# ── braw-bridge bauen ────────────────────────────────────────────────────────
log "Baue braw-bridge..."
BRAW_INSTALL="$CACHE_DIR/braw-install"
cmake -B "$ROOT_DIR/braw-bridge/build" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$BRAW_INSTALL" \
    -S "$ROOT_DIR/braw-bridge"
cmake --build "$ROOT_DIR/braw-bridge/build" -j"$(nproc)"
cmake --install "$ROOT_DIR/braw-bridge/build"

# ── r3d-bridge bauen ─────────────────────────────────────────────────────────
log "Baue r3d-bridge..."
R3D_INSTALL="$CACHE_DIR/r3d-install"
cmake -B "$ROOT_DIR/r3d-bridge/build" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$R3D_INSTALL" \
    -S "$ROOT_DIR/r3d-bridge"
cmake --build "$ROOT_DIR/r3d-bridge/build" -j"$(nproc)"
cmake --install "$ROOT_DIR/r3d-bridge/build"

# ── AppDir erstellen ─────────────────────────────────────────────────────────
log "Erstelle AppDir..."
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/sdk/Libraries/Linux"

# ── Native Binaries kopieren ─────────────────────────────────────────────────
cp "$ROOT_DIR/backend/target/release/proxy-generator-backend" "$APPDIR/usr/bin/"
cp "$BRAW_INSTALL/bin/braw-bridge"                            "$APPDIR/usr/bin/"
cp "$R3D_INSTALL/bin/r3d-bridge"                              "$APPDIR/usr/bin/"
chmod +x "$APPDIR/usr/bin/"*

# ── BRAW SDK Libs kopieren ───────────────────────────────────────────────────
# braw-bridge hat RPATH $ORIGIN/../sdk/Libraries/Linux
# → bei usr/bin/braw-bridge landet das in usr/sdk/Libraries/Linux/
BRAW_SDK="$ROOT_DIR/braw-bridge/sdk/Libraries/Linux"
for lib in libBlackmagicRawAPI.so libc++.so.1 libc++abi.so.1 \
           libDecoderCUDA.so libDecoderOpenCL.so \
           libInstructionSetServicesAVX2.so libInstructionSetServicesAVX.so; do
    [[ -f "$BRAW_SDK/$lib" ]] && cp "$BRAW_SDK/$lib" "$APPDIR/usr/sdk/Libraries/Linux/"
done

# ── Python extrahieren ───────────────────────────────────────────────────────
log "Extrahiere Python $PYTHON_VERSION..."
PYTHON_TMP="$CACHE_DIR/python-tmp"
rm -rf "$PYTHON_TMP"
mkdir -p "$PYTHON_TMP"
tar -xzf "$PYTHON_TAR" -C "$PYTHON_TMP"
# python-build-standalone entpackt in ein 'python/' Unterverzeichnis
cp -r "$PYTHON_TMP/python/." "$APPDIR/usr/"

PYTHON="$APPDIR/usr/bin/python3.12"
[[ -x "$PYTHON" ]] || die "Python-Binary nicht gefunden nach Entpacken"

# ── PyQt6 installieren (bundelt Qt6) ─────────────────────────────────────────
log "Installiere PyQt6..."
"$PYTHON" -m pip install --quiet PyQt6

# ── Frontend-Wheel bauen und installieren ────────────────────────────────────
log "Baue Frontend-Wheel..."
# Hatchling in temp-venv (nicht ins AppImage-Python)
BUILD_VENV="$CACHE_DIR/build-venv"
python3 -m venv "$BUILD_VENV"
"$BUILD_VENV/bin/pip" install --quiet hatchling

FRONTEND_DIST="$ROOT_DIR/frontend/dist"
rm -rf "$FRONTEND_DIST"
cd "$ROOT_DIR/frontend"
"$BUILD_VENV/bin/python" -m hatchling build -t wheel
WHEEL="$(ls "$FRONTEND_DIST"/proxy_generator-*.whl | head -1)"
[[ -f "$WHEEL" ]] || die "Frontend-Wheel nicht gefunden"
"$PYTHON" -m pip install --quiet --no-deps "$WHEEL"

# ── Desktop-Datei + Icon ─────────────────────────────────────────────────────
log "Kopiere Desktop-Integration..."
cp "$ROOT_DIR/flatpak/de.michaelproxy.ProxyGenerator.desktop" "$APPDIR/"

# SVG → PNG (256×256) für AppImage-Icon
rsvg-convert -w 256 -h 256 \
    "$ROOT_DIR/flatpak/de.michaelproxy.ProxyGenerator.svg" \
    -o "$APPDIR/de.michaelproxy.ProxyGenerator.png"

ln -sf de.michaelproxy.ProxyGenerator.png "$APPDIR/.DirIcon"

# ── AppRun kopieren ──────────────────────────────────────────────────────────
cp "$SCRIPT_DIR/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# ── AppImage packen ──────────────────────────────────────────────────────────
log "Erstelle AppImage..."
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 \
    "$APPIMAGETOOL" "$APPDIR" "$OUTPUT"

SIZE=$(du -sh "$OUTPUT" | cut -f1)
log "Fertig: $OUTPUT  ($SIZE)"
