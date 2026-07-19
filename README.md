# Tilt Paint

A small pressure- and tilt-sensitive drawing canvas built against the local
`../Hamerons_bevy` checkout. The master canvas lives in one CPU-backed RAM
buffer, so long strokes stay cheap and the eraser restores actual canvas pixels.
The display is split into 256×256 tiles, so a brush dab uploads only the touched
tiles rather than the entire 6.4 MB canvas.

The workspace selects the stable Rust toolchain from `rust-toolchain.toml`.
This is intentional: the 2026-07-11 Rust 1.99 nightly miscompiles Taffy 0.10.1's
compact layout lengths in optimized builds and makes Bevy UI panic at startup.

## Run

```bash
cargo run
```

## Controls

- Tablet pen contact: draw with pressure and tilt.
- Tablet eraser tip or barrel button: erase.
- Left mouse drag: draw with the pen fallback.
- Right mouse drag: erase with the mouse fallback.
- Shift + drag with either tool: resize that tool. Right/up grows; left/down shrinks.
- `C`: clear the canvas.

Pressure changes footprint size and opacity. Tilt stretches the oval toward the
reported tilt direction. The colored outline previews the live brush footprint.
