# Benchmarks

All quality scores come from [bench/benchmark.py](../bench/benchmark.py). It runs every clip
through `vidar-cli`, which uses the same chain as the live app (filter, model, de-clicker,
gate, limiter, at the default Balanced strength), and then scores the output. Higher is better
everywhere.

## Results

### DNS Challenge blind test set

600 real noisy recordings from Microsoft's
[DNS Challenge](https://github.com/microsoft/DNS-Challenge) (Interspeech 2020, `blind_test_set`).
There is no clean reference for real recordings, so they are scored by **DNSMOS P.835**:
Microsoft's neural model that predicts what human listeners would rate, on a 1-5 scale.

| | Speech (SIG) | Background (BAK) | Overall (OVRL) |
|---|---|---|---|
| Untouched | 2.96 | 2.46 | 2.22 |
| Voice | 3.13 | 3.89 | 2.81 |
| **Voice HD** | **3.30** | **3.96** | **2.99** |

- **SIG**: how natural the voice sounds. Both modes raise it slightly, so removing the noise
  does not damage the voice.
- **BAK**: how unobtrusive the background is. This is the biggest gain (+1.4 to +1.5). The
  model and the gate both contribute; how much each one adds is not measured yet (see below).
- **OVRL**: the overall impression, the closest single number to "is this better?".

### MS-SNSD

[MS-SNSD](https://github.com/microsoft/MS-SNSD): clean speech mixed with real recorded noise,
so the output can be compared to the original clean voice. The script picks 8 clips for each
of 9 noise types and mixes each at 0, 5, 10 and 20 dB SNR: 288 mixtures in total, with a fixed
random seed (7).

| | PESQ (quality, 1-4.5) | STOI (intelligibility, 0-1) |
|---|---|---|
| Untouched | 1.59 | 0.888 |
| Voice | 2.01 | 0.921 |
| **Voice HD** | **2.34** | **0.942** |

- **PESQ** (wideband, ITU-T P.862.2): how close the output sounds to the clean original.
- **STOI**: how many words a listener would understand. Both modes raise it, so the noise
  removal does not eat words.

### Processor use

A 120-second clip (the five golden test clips looped: clean speech, fan, keyboard, hiss, loud
voice) cleaned with `vidar-cli -j 1`, three runs each, median shown. Measured on an AMD Ryzen 7
9800X3D (8 cores, 16 threads) with the release build, RNNoise using its AVX2 layers. Both
models run single-threaded, so the time taken is the share of one core.

| | Speed (× real time) | One core | Whole CPU (Task Manager) |
|---|---|---|---|
| Voice | 41× | 2.4% | 0.15% |
| Voice HD, RNNoise speech detector (default) | 8.8× | 11.3% | 0.7% |
| Voice HD, spectral speech detector | 11× | 9.1% | 0.6% |
| Filter and gate only, no model | ~1000× | 0.1% | ~0% |

The time includes decoding and writing the files, which the no-model row shows is negligible.
In the default Voice HD, about a fifth of the work (2.2% of a core) is the RNNoise run that
only detects speech. That is what the spectral detector would save.

## Reading the numbers

- **Voice HD is better everywhere**, most clearly in loud and echoing rooms, at twice the
  delay (60 vs 30 ms) and about five times the processor use.
- **Voice gets most of the way** (OVRL 2.81 vs 2.99) at about 2.5% of one CPU core, so it
  stays the default.
- These are averages. Your mic and your room decide what sounds best, which is why the app has
  the Test tab.

## Running it yourself

You need the two data sets, the DNSMOS model and a release build.

```shell
cargo build --release -p vidar-cli
python -m venv .venv && .venv\Scripts\activate
pip install -r bench/requirements.txt

# MS-SNSD: clone github.com/microsoft/MS-SNSD
python bench/benchmark.py snsd --snsd-dir MS-SNSD --work bench-out

# DNS blind set: datasets/blind_test_set from the DNS-Challenge repo,
# interspeech2020/master branch; DNSMOS sig_bak_ovr.onnx from the same repo
python bench/benchmark.py dns --blind-dir blind_test_set --dnsmos sig_bak_ovr.onnx --work bench-out
```

`--systems` picks the configurations to score (default `voice,voice_hd`):

| System | `vidar-cli` flags |
|---|---|
| `voice` | `--engine rnnoise` |
| `voice_hd` | `--engine dpdfnet --dpdfnet-vad rnnoise` |
| `voice_hd_spectral` | `--engine dpdfnet --dpdfnet-vad spectral` |
| `voice_no_gate` | `--engine rnnoise --no-gate` |

The script prints averages overall, by SNR and noise type (MS-SNSD), or by recording type
(DNS). It writes every per-clip score to `bench-out/snsd_scores.csv` and
`bench-out/dns_scores.csv`. Processed audio is cached: a system is only re-run if any of its
output files are missing.

For MS-SNSD, `vidar-cli` trims its own delay so the output lines up with the reference. The
script checks this by reporting the median alignment lag, which should be about 0.

The processor figures need no data set: loop any 48 kHz recording to two minutes, then time
`vidar-cli --clean clip.wav out.wav -j 1` with each system's flags from the table above
(`--no-model` for the last row).

## Not measured yet

These are open questions about the chain. Each one is a single run of the script above:

- **Spectral speech detection for Voice HD** (`voice_hd_spectral`). If it scores as well as
  the RNNoise detector, Voice HD can stop running RNNoise alongside DPDFNet.
- **What the gate is worth** (`voice` vs `voice_no_gate`): how much of the BAK gain comes from
  the gate rather than the model.
- **The de-clicker after DPDFNet.** It is always on, but DPDFNet may already remove clicks.
  It needs a new system entry and a CLI flag to turn it off.
- **Processor use on a laptop CPU**: the figures above are from a fast desktop, and on a
  processor without AVX2 RNNoise falls back to its slower SSE4.1 or plain layers.
- **Delay**, measured end to end in the live app instead of the figures derived from the models.
- **The larger model** (`dpdfnet8_48khz_hr`) against the default `dpdfnet2_48khz_hr`.
