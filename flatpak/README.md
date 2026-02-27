# Flatpak Build-Anleitung

## Voraussetzungen

Folgende Tools muessen installiert sein:

- **flatpak-builder** -- Baut Flatpak-Apps aus Manifesten
- **flatpak-cargo-generator** -- Generiert Cargo-Quellen fuer Offline-Builds ([flatpak-builder-tools](https://github.com/niclasl/flatpak-builder-tools))
- **flatpak-pip-generator** -- Generiert Python-Quellen fuer Offline-Builds ([flatpak-builder-tools](https://github.com/niclasl/flatpak-builder-tools))

Runtimes und SDKs installieren:

```sh
flatpak install flathub org.kde.Platform//6.9 org.kde.Sdk//6.9
flatpak install flathub com.riverbankcomputing.PyQt.BaseApp//6.9
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable//24.08
```

## Quellen generieren

### Cargo-Quellen (Rust Backend)

```sh
python3 flatpak-cargo-generator.py ../backend/Cargo.lock -o flatpak/cargo-sources.json
```

### Python-Quellen (pip-Abhaengigkeiten)

```sh
python3 flatpak-pip-generator.py hatchling -o flatpak/python3-requirements
```

Falls weitere Python-Abhaengigkeiten in `frontend/pyproject.toml` hinzukommen,
diese ebenfalls an `flatpak-pip-generator.py` uebergeben.

## Bauen und Installieren

Aus dem Projekt-Stammverzeichnis:

```sh
# Bauen
flatpak-builder --force-clean build-dir flatpak/de.michaelproxy.ProxyGenerator.yml

# Ins lokale Repo exportieren und installieren
flatpak-builder --user --install --force-clean build-dir flatpak/de.michaelproxy.ProxyGenerator.yml
```

## Starten

```sh
flatpak run de.michaelproxy.ProxyGenerator
```
