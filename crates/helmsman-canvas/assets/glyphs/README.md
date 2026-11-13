# Glyphs

One SVG a glyph, 64 by 64 units, black shapes on nothing: only the
alpha counts, the app tints it in the theme's colours. The file's name is
the glyph's name (`push-button.svg` is `push-button`); the build step
(crates/helmsman-canvas/build.rs) rasterizes every file here into an alpha
mask baked into the app, so redrawing a glyph is replacing its file and
rebuilding, with no code change. Add a glyph the same way, then name it
where it is used (`glyph_lib::for_point`, the palette).

The ones here are stencils in the spirit of emoji, placeholders until the
real set is drawn (Nick, 2026-11-12).
