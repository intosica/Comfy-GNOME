# GNOME-ComfyUI

Native GTK4/Libadwaita frontend for [ComfyUI](https://github.com/comfyanonymous/ComfyUI), written in Rust. Targets Linux/X11.

Status: Phase 1 (shell + splash + canvas prototype). See `ROADMAP.md` and `AGENT-HANDOFF.md`.

## Build

```sh
# native (needs GTK >= 4.14, libadwaita >= 1.5 dev packages)
cargo run --release

# flatpak (GNOME 50 runtime + rust-stable SDK extension)
flatpak-builder --user --install --force-clean flatpak-build build-aux/io.github.gnomecomfyui.GnomeComfyUI.json
flatpak run io.github.gnomecomfyui.GnomeComfyUI
```

## License

GPL-3.0-only. Canvas design adapted from [Helvum](https://gitlab.freedesktop.org/pipewire/helvum) (GPL-3.0-only);
visual constants from ComfyUI_frontend / litegraph (GPL-3.0). See `docs/TRANSLATION-NOTES.md`.
