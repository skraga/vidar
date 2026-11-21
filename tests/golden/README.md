# Reference vectors

Stored output of the signal chain, used by `crates/vidar-denoise/tests/golden.rs` to catch any
change in how Vidar processes audio.

| Path | What |
|---|---|
| `source/*.wav` | Raw speech (Windows TTS voices, 48 kHz mono 16-bit) |
| `clips/*.wav` | Test inputs: the speech mixed with seeded synthetic fan noise, keyboard clicks and hiss |
| `<config>/<clip>.csv` | Per-frame reference values: speech probability, input levels, gate gain and state, noise floor, output peak and RMS |
| `<config>/<clip>.f32` | Full output samples (little-endian float32) for two cases |
| `manifest.json` | The configurations and clips |

The test allows about 1e-3 (−65 dB) on levels and output samples and 5e-3 on speech
probability, because RNNoise picks scalar, SSE4.1 or AVX2 code depending on the CPU and the
rounding differences build up in its recurrent state. The gate must make exactly the same
open/close decisions on every frame.

The vectors were first written before the first commit, by running the chain over the clips
with the settings in `manifest.json` and dumping one CSV row per 480-sample frame (columns as
in the header row). If a change to the chain is intentional, regenerate them with

```shell
VIDAR_BLESS_GOLDEN=1 cargo test -p vidar-denoise --release --test golden -- --nocapture
```

which rewrites only the files that moved past the bounds (the others keep their stored
values, so this CPU's RNNoise rounding does not churn them) and prints the report. Review the
report and the diff before committing.
