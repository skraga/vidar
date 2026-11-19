# Third-party notices

Vidar is MIT-licensed (see [LICENSE](LICENSE)). It includes, downloads or bundles the third-party
components listed below, which keep their own licences. Each section gives the source, version,
licence, copyright holder and what the licence asks of anyone redistributing Vidar.

Last checked: 2026-11-16. Licences were read from the files in this repository and from the
upstream pages linked in each section.

## Notes for anyone redistributing Vidar

A Vidar release (`cargo xtask dist`) ships the licence texts each component needs, in
`licenses\` next to `vidar.exe`; see "What a release ships" at the end. The items below put
conditions on redistribution beyond shipping licence texts.

1. **VB-CABLE is donationware and must stay identifiable as VB-Audio's.**
   - VB-Audio's licensing page ([vb-audio.com/Services/licensing.htm](https://vb-audio.com/Services/licensing.htm),
     section "VB-CABLE Distribution with another product") allows bundling and silent install:
     > "We allow the distribution of the VB-CABLE package AS IS together with your application
     > (freeware or commercial software). VB-CABLE may also be embedded in your installer and
     > deployed through a silent installation process, provided that our donationware model
     > remains applicable. The donationware model is considered applicable when the End User can
     > clearly identify VB-CABLE as a VB-Audio application and has the opportunity to support the
     > project by making a donation or purchasing a license if they find it useful. To comply with
     > this requirement, your documentation, installer, website, or application should clearly
     > mention at least: The origin of VB-CABLE : www.vb-cable.com . VB-CABLE is a donationware,
     > all participations are welcome."
   - The `readme.txt` shipped inside the package (`drivers/vbcable/readme.txt`) is stricter:
     > "It is not allowed to integrate the VB-CABLE package in another software installation
     > procedure without Author agreement."

     The web page is newer and more specific, but the two texts differ. Vidar runs
     `VBCABLE_Setup_x64.exe -i -h` (silent install) from its own UI
     (`crates/vidar-win/src/driver.rs`).
   - Vidar renames the cable endpoints to "Vidar AI Microphone" / "Vidar Internal" and changes
     their icon (`crates/vidar-win/src/branding.rs`). The "(VB-Audio Virtual Cable)" interface
     name is kept.
   - How Vidar meets the attribution condition: the About tab has a VB-CABLE card that says
     "VB-CABLE by VB-Audio Software", "Donationware", states that Vidar AI Microphone and Vidar
     Internal are VB-CABLE devices under Vidar's names, says "All participations are welcome",
     and links to www.vb-cable.com and VB-Audio's support page (`apps/vidar/src/ui.rs`). The
     install prompt and Settings name the driver as VB-CABLE by VB-Audio, donationware.
   - Corporate or institutional deployments need paid licences: "If VB-CABLE is distributed in a
     professional environment (companies, institutions, organizations, public administrations,
     etc.) where end users or employees are not in a position to identify VB-CABLE independently
     or contribute to its funding, the distributor must purchase the appropriate number of
     VB-CABLE licenses directly from our webshop." Do not deploy Vidar to organisations without
     buying VB-CABLE licences.
   - The vendored files are byte-identical to the official `VBCABLE_Driver_Pack46.zip` (all 31
     files' SHA-256 matched on 2026-11-16). Keep them unmodified.

2. **ONNX Runtime's notices travel with `onnxruntime.dll`.** The DLL incorporates many
   third-party components. Keep `licenses\onnxruntime-LICENSE.txt` and
   `licenses\onnxruntime-ThirdPartyNotices.txt` with it.

3. **DPDFNet model revision.** The models are Apache-2.0. `xtask` downloads them from the
   `main` branch and checks a SHA-256, so an upstream change would break the fetch but could not
   silently change what is shipped. The shipped files match Hugging Face revision
   `dd6818d00f50c836fed43a6243ebe49116de5964`.

4. **Benchmark datasets with non-commercial licences (not shipped).** `bench/fetch.py`
   downloads EARS (CC BY-NC 4.0) and WHAM! noise (CC BY-NC 4.0) for local evaluation. Nothing
   from them is in the repository or in Vidar binaries. Do not commit or publish clips or
   mixtures derived from them, such as MUSHRA listening-test files. Publishing aggregate
   scores is fine.

---

## VB-CABLE (VB-Audio Virtual Cable)

| | |
|---|---|
| Location | `drivers/vbcable/` (committed to git; redistributed with Vidar) |
| Source | https://vb-audio.com/Cable/ (`VBCABLE_Driver_Pack46.zip`) |
| Version | Driver 3.5.2.0 (`DriverVer = 10/31/2026`), setup 2.1.5.8 (x64) / 2.1.5.7 (x86), control panel 3.3.2.0 |
| Licence | Proprietary donationware. End-user licence in `drivers/vbcable/readme.txt`; distribution terms at https://vb-audio.com/Services/licensing.htm |
| Copyright | Copyright V. Burel 2010-2026, all rights reserved (Vincent Burel / VB-Audio Software, France). Binaries signed by "BUREL VINCENT Entrepreneur individuel"; `.sys` files carry Microsoft's WHQL signature |

**Requirements:**
- Redistribute the package unmodified ("AS IS"). Do not remove files, including `readme.txt`.
- Mention the origin (www.vb-cable.com) and that VB-CABLE is donationware ("all participations
  are welcome") in the documentation, installer, website or app.
- The end user must be able to identify VB-CABLE as a VB-Audio product and to donate or buy a
  licence.
- Distribution in professional environments requires buying licences from VB-Audio.
- Only the plain VB-CABLE may be bundled. VB-CABLE A+B and C+D "cannot be distributed or
  bundled with other product".
- See **Notes for anyone redistributing Vidar, item 1**.

## RNNoise

| | |
|---|---|
| Location | `third_party/rnnoise/` (C source, statically linked into Vidar) |
| Source | https://github.com/xiph/rnnoise |
| Version | Commit `70f1d256acd4b34a572f999a05c87bf00b67730d` (`third_party/rnnoise/UPSTREAM_COMMIT`) |
| Licence | BSD-3-Clause (`third_party/rnnoise/COPYING`) |
| Copyright | Copyright (c) 2007-2017, 2024 Jean-Marc Valin; Copyright (c) 2023 Amazon; Copyright (c) 2017, Mozilla; Copyright (c) 2005-2017, Xiph.Org Foundation; Copyright (c) 2003-2004, Mark Borgerding (KISS FFT) |

**Model weights:** `rnnoise_data.c` / `rnnoise_data.h`, downloaded by `cargo xtask fetch` from
`https://media.xiph.org/rnnoise/models/rnnoise_data-0a8755f8e2d834eff6a54714ecc7d75f9932e845df35f8b59bc52a7cfe6e8b37.tar.gz`
and checked against that SHA-256 (`third_party/rnnoise/model_version`). Xiph publishes them as
part of RNNoise under the same BSD-3-Clause licence. The weights are compiled into the Vidar
binary.

**Requirements:** binary distributions must reproduce the copyright notice, the three
conditions and the disclaimer in their documentation or other materials. Ship `COPYING`. The
names of Xiph.Org and its contributors may not be used to endorse Vidar without permission.

## ONNX Runtime

| | |
|---|---|
| Location | `third_party/onnxruntime/` (not in git; fetched by `cargo xtask fetch`, shipped as `onnxruntime.dll`) |
| Source | Microsoft's NuGet package [Microsoft.ML.OnnxRuntime 1.32.0](https://www.nuget.org/packages/Microsoft.ML.OnnxRuntime/1.32.0) (`runtimes/win-x64/native/onnxruntime.dll`, `LICENSE`, `ThirdPartyNotices.txt`); the same Microsoft-signed DLL as the GitHub release zip, which is no longer available |
| Version | 1.32.0 (`ORT_VERSION` in `xtask/src/main.rs`; package SHA-256 `a9150bc2...5641db`) |
| Licence | MIT (`third_party/onnxruntime/LICENSE`), plus third-party components listed in `third_party/onnxruntime/ThirdPartyNotices.txt` |
| Copyright | Copyright (c) Microsoft Corporation |

`ThirdPartyNotices.txt` (about 340 KB, about 70 entries) covers components such as protobuf,
zlib, ONNX, Eigen (MPL-2.0), oneDNN, abseil, flatbuffers, re2, XNNPACK, cpuinfo, mimalloc,
SafeInt, Boost and others, under MIT, BSD, Apache-2.0, MPL-2.0, Boost, zlib and similar
licences. It also says Microsoft offers the corresponding source code at
http://3rdpartysource.microsoft.com and that LGPL-licensed parts may be reverse engineered for
debugging.

**Requirements:** include the MIT copyright and permission notice, and ship
`ThirdPartyNotices.txt` unchanged next to `onnxruntime.dll`. The Rust bindings `ort` /
`ort-sys` 2.0.0-rc.15 are MIT OR Apache-2.0 and are covered under "Rust crate dependencies".

## DPDFNet models

| | |
|---|---|
| Location | `models/` (not in git; fetched by `cargo xtask fetch`) |
| Source | https://huggingface.co/Ceva-IP/DPDFNet (files `onnx/dpdfnet2_48khz_hr.onnx` and `onnx/dpdfnet8_48khz_hr.onnx`); code at https://github.com/ceva-ip/DPDFNet |
| Version | Hugging Face repo revision `dd6818d00f50c836fed43a6243ebe49116de5964` (last modified 2026-07-05). SHA-256 values: `dpdfnet2_48khz_hr` `7f0575a5...91dc14b`, `dpdfnet8_48khz_hr` `7b3afbb2...c5f5631` |
| Licence | **Apache-2.0**. The model card declares `license: apache-2.0` and ends with "## License — Apache-2.0"; the GitHub repo has the full Apache-2.0 `LICENSE` file. No NOTICE file and no use restrictions (non-commercial or otherwise) were found. |
| Copyright | Ceva Inc. (Ceva-IP). Neither the model card nor the LICENSE names a copyright holder; the LICENSE appendix still reads "Copyright [yyyy] [name of copyright owner]". |

Paper: D. Rika, N. Sapir, I. Gus, "DPDFNet: Boosting DeepFilterNet2 via Dual-Path RNN", 2025,
arXiv:2512.16420. According to the paper, the models were trained on the DNS Challenge 4 data
plus Multilingual LibriSpeech. Both are released under permissive CC BY 4.0-style terms, and
their licences do not carry over to the published weights.

**Requirements (Apache-2.0 section 4):** give recipients a copy of the Apache-2.0 licence,
keep attribution notices, and state any changes if the model files are modified. Vidar ships
them unmodified.

## Fonts

### Inter (bundled in the `vidar` executable)

| | |
|---|---|
| Location | `assets/fonts/Inter-Regular.ttf`, `assets/fonts/Inter-SemiBold.ttf` (embedded with `include_bytes!` in `apps/vidar/src/ui.rs`) |
| Source | https://github.com/rsms/inter |
| Version | 4.000 (git-a52131595) |
| Licence | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`) |
| Copyright | Copyright (c) 2016 The Inter Project Authors (https://github.com/rsms/inter). "Inter" is a trademark of rsms. |

**Requirements:** ship the copyright notice and the OFL text with the font. Embedding the font
in software is allowed. The font may not be sold on its own. Modified versions may not use the
name "Inter".

### egui default fonts (crate `epaint_default_fonts` 0.37.0, enabled through eframe's `default_fonts` feature)

| Font | Licence | Copyright |
|---|---|---|
| Hack Regular | MIT, plus the Bitstream Vera License for the Vera-derived glyphs (Reserved Font Names "Bitstream" and "Vera"); the DejaVu changes are public domain | Copyright (c) 2018 Source Foundry Authors; Bitstream Vera Sans Mono Copyright 2003 Bitstream Inc. |
| Ubuntu Light | Ubuntu Font Licence 1.0 | Copyright Canonical Ltd. |
| Noto Emoji Regular | SIL OFL 1.1 | Copyright Google Inc. |
| emoji-icon-font, egui-icons (a subset of it) | MIT | Copyright (c) 2014 John Slegers |

Source: https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts. The licence texts
come with the crate (`fonts/OFL.txt`, `fonts/UFL.txt`, `fonts/Hack-Regular.txt`,
`fonts/emoji-icon-font-mit-license.txt`).

**Requirements:** include each copyright notice and licence text. Under the OFL and UFL, the
fonts may not be sold on their own, and modified versions may not use the reserved names.

## Rust crate dependencies

`cargo deny list -l license` (graph target `x86_64-pc-windows-msvc`, all features, run
2026-11-16) reports about 230 third-party crates. Many are dual-licensed, so they appear under
more than one licence below:

| Licence | Crates | Notes |
|---|---:|---|
| MIT | 199 | Most of the tree; usually "MIT OR Apache-2.0" |
| Apache-2.0 | 185 | Usually as the alternative to MIT |
| Zlib | 10 | bytemuck, glow, miniz_oxide, raw-window-handle, zlib-rs, foldhash, ... |
| ISC | 6 | libloading, ring, rustls, rustls-webpki, untrusted |
| BSD-3-Clause | 3 | moxcms, pxfm, subtle |
| **MPL-2.0** | 13 | **symphonia** 0.6.1 and its codec/format crates (used by `vidar-cli` for file decoding) |
| 0BSD | 2 | adler2, audio-codec-algorithms |
| Unlicense | 2 | memchr, byteorder-lite (each dual-licensed with MIT) |
| Unicode-3.0 | 2 | unicode-ident, unicode-properties (data tables) |
| OFL-1.1 / Ubuntu-font-1.0 | 1 | epaint_default_fonts (see Fonts) |
| BSD-2-Clause, CC0-1.0, MIT-0 | 1 each | zerocopy, constant_time_eq (each dual- or triple-licensed) |
| CDLA-Permissive-2.0 | 1 | webpki-roots (xtask only, not shipped) |
| GPL-2.0-only | 1 | self_cell, licensed "Apache-2.0 OR GPL-2.0-only"; Vidar uses it under Apache-2.0 |

`cargo deny check licenses` passes against the allow-list in `deny.toml`. `ureq`, `rustls`,
`ring`, `webpki-roots`, `tar` and `zip` are build-tool (xtask) dependencies and are not part of
the shipped executables. No copyleft licence applies to the shipped binaries except MPL-2.0.

**MPL-2.0 (symphonia):** file-level copyleft. Vidar uses symphonia unmodified, so the only
obligations are to tell recipients that the MPL-2.0 code is available in source form (at
https://github.com/pdeljanov/Symphonia and on crates.io) and to include the MPL-2.0 text. If
symphonia's source files are ever modified, those files must be published under MPL-2.0.

**Requirements (all crates):** MIT, BSD, ISC, Zlib and Apache-2.0 require the copyright
notices and licence texts to accompany binary distributions. The per-crate texts are not
maintained by hand: `cargo xtask dist` generates them with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) (`about.toml`, `about.hbs`, same
accepted-licence list as `deny.toml`) into `licenses\THIRD_PARTY_LICENSES.html`. cargo-about
picks up the actual licence files from each crate, including copyright lines and any Apache
NOTICE files. CI runs it on every push.

## Other assets in the repository

### Golden test audio (`tests/golden/`)

Test-only. These files are not part of Vidar binaries.

- `source/david.wav`, `source/zira.wav`: synthetic speech generated with the built-in Windows
  text-to-speech voices (Microsoft David, Microsoft Zira), according to `tests/golden/README.md`.
  The voices belong to Microsoft and are licensed as part of Windows. Using their output as test
  data in this repository is low-risk, but the clips are not covered by Vidar's MIT licence.
- `clips/*.wav`: the speech above mixed with seeded synthetic noise (fan, keyboard clicks,
  hiss) generated for this project. No clip comes from VoiceBank+DEMAND, MS-SNSD, DNS or any
  other third-party speech or noise corpus.
- `*/*.csv`, `*/*.f32`: reference outputs computed by Vidar.

### Benchmark datasets (`bench/fetch.py`; downloaded locally, not in the repository)

| Dataset | Source | Licence |
|---|---|---|
| VoiceBank+DEMAND (48 kHz test set) | https://datashare.ed.ac.uk (Valentini-Botinhao et al., 2016) | CC BY 4.0 |
| DNS Challenge 5 blind test set | https://dnschallengepublic.blob.core.windows.net/dns5archive/ | CC BY 4.0 (Microsoft DNS Challenge) |
| EARS | https://github.com/facebookresearch/ears_dataset | **CC BY-NC 4.0** ("CC-NC 4.0 International") |
| WHAM! high-res noise | WHAM! (Wichern et al.) | **CC BY-NC 4.0** |

Only scores derived from these datasets appear in `docs/benchmarks.md`. If audio examples
derived from them are ever published, credit the dataset and keep the non-commercial terms for
EARS and WHAM!.

### Application icon (`assets/vidar-icon.png`)

Project artwork for Vidar, covered by the project's MIT licence. No third-party source was
found.

---

## What a release ships

`cargo xtask dist` copies these into `dist\vidar` and fails if any is missing:

| File | Covers |
|---|---|
| `LICENSE` | Vidar (MIT) |
| `THIRD_PARTY_NOTICES.md` | this file |
| `drivers\vbcable\readme.txt` | VB-CABLE end-user licence (inside the unmodified package) |
| `licenses\rnnoise-COPYING.txt`, `licenses\rnnoise-AUTHORS.txt` | RNNoise and its weights (BSD-3-Clause) |
| `licenses\onnxruntime-LICENSE.txt`, `licenses\onnxruntime-ThirdPartyNotices.txt` | ONNX Runtime and the components in it |
| `licenses\dpdfnet-Apache-2.0.txt` | DPDFNet models (Apache-2.0) |
| `licenses\inter-OFL.txt` | Inter font (SIL OFL 1.1) |
| `licenses\egui-fonts-*.txt` | egui's default fonts: Noto Emoji (OFL), Ubuntu Light (UFL), Hack, emoji-icon-font and egui-icons (MIT) |
| `licenses\symphonia-MPL-2.0.txt` | symphonia (MPL-2.0); source at https://github.com/pdeljanov/Symphonia |
| `licenses\THIRD_PARTY_LICENSES.html` | every Rust crate, generated by cargo-about |

The vendored texts come from `third_party/licenses/` (see its README for their sources).
