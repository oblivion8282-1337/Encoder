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
