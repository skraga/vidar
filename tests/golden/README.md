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

If a change to the chain is intentional, the vectors must be regenerated and the change
reviewed: the test tolerances are tight (about 1e-6) on purpose.
