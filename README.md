# Stroke Drawing Test

A pressure-sensitive drawing test for the local `hamerons_stroke_render` and
`vector_stroke_render` crates. The app starts in a renderer menu and uses Bevy
states to activate exactly one backend at a time:

- **Hamerons Paint Renderer** uses the existing GPU paint/tile backend and
  persists documents as `stroke_lab.kra`.
- **Vector Stroke Renderer** keeps editable pressure-sensitive stroke paths and
  persists them as `stroke_lab.ink.ron`.

Both documents remain in memory when returning to the menu, so switching
renderers does not mix their output or throw away the current session.

## Run

```bash
cargo run --release
```

## Controls

- Menu: choose a renderer with mouse or pen contact, or press `1` / `2`.
- Tablet pen contact: draw with pressure and tilt.
- Tablet eraser tip: erase.
- Left mouse drag: draw.
- Right mouse drag: erase.
- `Shift` + drag: change active tool size.
- `[` / `]`: decrease or increase the active tool size.
- `C`: clear the current canvas.
- `Ctrl+Z`: undo; `Ctrl+Shift+Z` or `Ctrl+Y`: redo.
- `V`: toggle low-latency presentation and vsync.
- `Ctrl+S` / `Ctrl+O`: save or load the selected backend's document format.
- `N`: create and select a normal layer.
- `Page Up` / `Page Down`: select the layer above or below.
- `Shift+Page Up` / `Shift+Page Down`: reorder the active layer.
- `H`: toggle active-layer visibility.
- `,` / `.`: reduce or increase active-layer opacity.
- `Escape`: return to the renderer menu.

The advanced color selector at the upper right uses a hue ring and a
saturation/value triangle. Drag either with the mouse or tablet pen; releasing
commits an immutable material for subsequent strokes.

Paint mode also provides two pointer-friendly panels:

- **Layers** selects, creates, reorders, hides, and changes the opacity of paint
  layers. Its **Paint Mix** control switches the active document surface
  between normal RGBA paint and the native two-plane pigment model. A document
  keeps one paint model because the models use different persistent GPU plane
  layouts, so the selector will not reinterpret strokes already recorded with
  the other model.
- **Brush** selects paint, eraser, or a dedicated local smear tool; changes
  size and flow; and controls pigment wetness. Dragging with **Smear** moves
  existing wet pigment under the brush without adding color. The separately
  labeled **Global Smear** controls apply the non-destructive effect across the
  whole pigment surface.

The Hamerons preview uses its pressure/tilt brush footprint. The vector preview
shows the pressure-adjusted path width; pressure and tilt samples are retained
in the RON document for later editing or export.
