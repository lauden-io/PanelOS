# Vendored crates

Crates used in a modified form, through `[patch.crates-io]` in `app/Cargo.toml`.

## i-slint-renderer-skia 1.19.0

Slint's Skia renderer (GPL-3.0-only, among other licences; see its `LICENSES`),
used by the GPU build. Change:

- `itemrenderer.rs`, `draw_image_impl`: the image origin is no longer rounded to
  whole device pixels, so images moving slowly (the weather scenes' drifting sky)
  move smoothly instead of a pixel at a time (`PANELOS_ALIGN_IMAGES=1` brings
  the upstream rounding back, for measurements).

Update: take the same version from crates.io and apply the change again (it is
marked with a `PanelOS:` comment).
