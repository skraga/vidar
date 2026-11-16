# Licence texts shipped with Vidar

Unmodified licence texts that `cargo xtask dist` copies into `dist\vidar\licenses\`, for
components whose licence text is not otherwise in the repository. See THIRD_PARTY_NOTICES.md.

| File | Covers | Copied from |
|---|---|---|
| `Apache-2.0.txt` | DPDFNet models (Ceva Inc.) | crate `ort` 2.0.0-rc.15, `LICENSE-APACHE` (standard Apache-2.0 text) |
| `MPL-2.0.txt` | symphonia and its codec/format crates | crate `symphonia` 0.6.1, `LICENSE` |
| `egui-fonts-OFL.txt` | Noto Emoji | crate `epaint_default_fonts` 0.37.0, `fonts/OFL.txt` |
| `egui-fonts-UFL.txt` | Ubuntu Light | same crate, `fonts/UFL.txt` |
| `egui-fonts-Hack.txt` | Hack Regular | same crate, `fonts/Hack-Regular.txt` |
| `egui-fonts-emoji-icon-font-MIT.txt` | emoji-icon-font | same crate, `fonts/emoji-icon-font-mit-license.txt` |
| `egui-fonts-egui-icons.txt` | egui-icons (subset of emoji-icon-font) | same crate, `fonts/egui-icons.txt` |

Inter's licence is `assets/fonts/OFL.txt`. Per-crate texts for all Rust dependencies are
generated at build time by cargo-about (`about.toml`, `about.hbs`).
