# Comfy-GNOME

Native GTK4/Libadwaita frontend for [ComfyUI](https://github.com/comfyanonymous/ComfyUI), written in Rust. Targets Linux/X11.

Status: Phase 1 (shell + splash + canvas prototype). See `ROADMAP.md` and `AGENT-HANDOFF.md`.

## Run

```sh
./launch-system.sh           # native release build (GTK >= 4.14, libadwaita >= 1.5 dev packages); rebuilds when sources change
./launch-flatpak.sh          # Flatpak (GNOME 50 runtime + rust-stable SDK extension); built + installed on first run
./launch-flatpak.sh --rebuild  # rebuild + reinstall the Flatpak after code changes
cargo run                    # debug build for development
```

On the splash screen pick your ComfyUI folder (auto-detected in `~/ComfyUI`, `~/Documents/ComfyUI`, …) and press
**Launch ComfyUI**: it runs `main.py` with the folder's `venv`/`.venv`. Alternatively pick your own launch script
(browsers it tries to open are blocked by default), or **Skip** to attach to an already running backend.
Flatpak and native behave the same: in the Flatpak, ComfyUI is run on the host via `flatpak-spawn --host`.

## License

GPL-3.0-only. Canvas design adapted from [Helvum](https://gitlab.freedesktop.org/pipewire/helvum) (GPL-3.0-only);
visual constants from ComfyUI_frontend / litegraph (GPL-3.0). See `docs/TRANSLATION-NOTES.md`.
Bundled symbolic icons in `data/icons/` are from the Adwaita icon theme (CC-BY-SA 3.0 / LGPL-3.0).
