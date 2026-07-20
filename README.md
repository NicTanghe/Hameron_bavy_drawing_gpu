# Hamerons Stroke Lab

A small pressure-sensitive drawing test for the local
`hamerons_stroke_render` crate. The visual shell follows the earlier
`drawing_test`: a paper-colored full-window canvas, low-latency presentation,
a live brush outline, and a compact status HUD.

Unlike the earlier prototype, this app does not contain a paint canvas, CPU
pixel buffer, tile uploader, shader, or custom render pipeline. It installs
`HameronsStrokeRenderPlugin`; the engine owns stroke history, the active GPU
overlay, persistent tile replay, erasing, and cache diagnostics. The only
input implemented by the app is a small mouse fallback that appends geometry to
the engine's public `StrokeDocument` API. Tablet input goes directly through the
engine's pen adapter.

## Run

```bash
cargo run --release
```

## Controls

- Tablet pen contact: draw with pressure.
- Tablet eraser tip: erase.
- Left mouse drag: draw through the engine's document API.
- Right mouse drag: erase through the engine's document API.
- `[` / `]`: decrease or increase the active tool size.
- `C`: clear without discarding vector history.
- `Ctrl+Z`: undo; `Ctrl+Shift+Z` or `Ctrl+Y`: redo.
- `V`: toggle low-latency presentation and vsync.
- `Ctrl+S`: checkpoint atomically to `stroke_lab.kra` on a background worker.
- `Ctrl+O`: validate and load `stroke_lab.kra`.
- `N`: create and select a normal layer.
- `Page Up` / `Page Down`: select the layer above or below.
- `Shift+Page Up` / `Shift+Page Down`: reorder the active layer.
- `H`: toggle active-layer visibility.
- `,` / `.`: reduce or increase active-layer opacity.

The cursor's colored ring shows the engine's pressure-adjusted round footprint.
Its short direction tick reports tablet tilt captured by the engine, although
the current coverage shader still renders round strokes.
