# Vendored crates

Crates used in a modified form, through `[patch.crates-io]` in `app/Cargo.toml`.

## i-slint-renderer-skia 1.19.0

Slint's Skia renderer (GPL-3.0-only, among other licences; see its `LICENSES`),
used by the GPU build. Changes:

- `itemrenderer.rs`, `draw_image_impl`: the image origin is no longer rounded to
  whole device pixels, so images moving slowly (the weather scenes' drifting sky)
  move smoothly instead of a pixel at a time (`PANELOS_ALIGN_IMAGES=1` brings
  the upstream rounding back, for measurements).
- `itemrenderer.rs`, `render_and_blend_layer`: a cached layer (`cache-rendering-hint`)
  that moves is drawn at its exact sub-pixel position along the axes it moves on
  (linear filtering), so a scrolling title or the wandering clock glides; a still
  layer is rounded to whole pixels as upstream, so it is no softer at rest
  (`PANELOS_ALIGN_LAYERS=1` always rounds, for measurements).
- `itemrenderer.rs`, `draw_text`, `draw_text_input` and `draw_glyph_run`: the same
  for text: a moving text's origin is not rounded along the axes it moves on (Skia
  keeps glyphs on quarter pixels across), and a vertical move turns Skia's baseline
  snap off for its glyphs (with the snap on, Skia rounds their y to whole pixels);
  a still text is drawn as upstream (`PANELOS_ALIGN_TEXT=1` always rounds, for
  measurements).
- `panelos_motion.rs` (new), used from `itemrenderer.rs` (`moving_axes` and the
  `_axes` variants of the two rounding helpers) and `lib.rs` (the cache in
  `SkiaRenderer`, cleared with the other caches and per destroyed component): the
  last device position of each layer and text item drawn. An axis counts as moving
  while its position changed in two draws less than 0.2 s apart, the last one less
  than 0.2 s ago (a single jump, as when a layout changes, does not count);
  meanwhile the renderer asks for another frame, so the item is drawn rounded again
  once it has stopped. No allocation per frame (an entry per item, made when it is
  first drawn).

Update: take the same version from crates.io and apply the changes again (each is
marked with a `PanelOS:` comment; `panelos_motion.rs` is PanelOS's own file).
