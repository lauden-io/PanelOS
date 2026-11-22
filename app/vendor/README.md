# Vendored crates

Crates used in a modified form, through `[patch.crates-io]` in `app/Cargo.toml`.

## i-slint-renderer-skia 1.18.1

Slint's Skia renderer (GPL-3.0-only, among other licences; see its `LICENSES`),
used by the GPU build. Changes:

- `itemrenderer.rs`, `draw_image_impl`: the image origin is no longer rounded to
  whole device pixels, so images moving slowly (the home page's drifting sky)
  move smoothly instead of a pixel at a time (`PANELOS_ALIGN_IMAGES=1` brings
  the upstream rounding back, for measurements).
- `lib.rs`, `free_graphics_resources`: the layer cache (`cache-rendering-hint`)
  also forgets a destroyed component. Without it a component created later at
  the same address (a page swapped for another one) showed the old layer.

Update: take the same version from crates.io and apply the changes again (each
is marked with a `PanelOS:` comment).
