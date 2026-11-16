# Benchmarks

All quality scores come from [bench/benchmark.py](../bench/benchmark.py). It runs every clip
through `vidar-cli`, which uses the same chain as the live app (filter, model, de-clicker,
gate, limiter, at the default Balanced strength), and then scores the output.

Every average comes with a 95% confidence interval, and every difference is checked with a
paired bootstrap test (10,000 resamples of the same clips). The tables only call a difference
real when that test does.

## Results

### VoiceBank+DEMAND, 48 kHz

The standard test set in speech enhancement papers:
[824 clips](https://datashare.ed.ac.uk/handle/10283/2791) of two speakers with real DEMAND
noise, recorded at 48 kHz with a clean take of each. Because it is full band, it is the one
set here that can show what Voice HD does above 8 kHz.

| | PESQ | ESTOI | NISQA | DNSMOS OVRL | Word errors | Word starts cut |
|---|---|---|---|---|---|---|
| Clean reference | | | 4.62 | 3.22 | 1.4% | |
| Untouched | 1.97 | 0.787 | 3.00 | 2.68 | 2.5% | 0.0% |
| Voice | 2.12 | 0.770 | 3.32 | 2.99 | **5.5%** | **13.7%** |
| **Voice HD** | **2.65** | **0.826** | **4.74** | **3.11** | 2.6% | 0.4% |
| Voice HD, no gate | 2.82 | 0.844 | 4.70 | 3.10 | 2.6% | 0.3% |

- **PESQ** (wideband, ITU-T P.862.2, 1-4.5) and **ESTOI** (intelligibility, 0-1) compare the
  output with the clean take. ESTOI is the newer form of STOI and copes better with noise that
  comes and goes.
- **NISQA** (1-5) is a neural model that predicts what listeners would say, like DNSMOS, but it
  listens to the full 48 kHz band. Voice HD scores slightly above the clean takes, which have
  some noise of their own.
- **Word errors**: Whisper (large-v3-turbo) transcribes every clip and is checked against the
  data set's own transcripts. This is the most direct test of "did it eat words".
- **Word starts cut**: the share of word starts (after a pause of 100 ms or more) whose first
  50 ms come out more than 6 dB quieter than the rest of the word. This is what a gate that
  opens late, or a model that is slow to trust a new sound, does.

What it shows:

- **Voice HD is clearly better than Voice on every score here.**
- **Voice costs words.** It doubles the word error rate (2.5% → 5.5%), lowers ESTOI below the
  untouched input, and cuts about one word start in seven. NISQA's sub-scores also rate it as
  choppier and more coloured than the input. Turning the gate off changes none of this, so it
  is RNNoise itself.
- **Voice HD keeps words.** Its word error rate is the same as the untouched input's.
- **The gate does little for Voice HD here.** Without it, PESQ and ESTOI are better (2.82 vs
  2.65) and the background scores are the same within 0.03. See
  [the gate on its own](#noise-with-no-speech) for where it does help.

Two scores are left out of the table on purpose. **SI-SDR** falls for every configuration,
Voice HD included, because the leveler and gate change the gain over time and SI-SDR counts any
gain change as error. **Log-spectral distance** above 8 kHz is better with Voice HD (11.0 dB vs
12.0 untouched) and worse with Voice (13.5). Both are in the CSV.

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
- **BAK**: how unobtrusive the background is. This is the biggest gain (+1.4 to +1.5).
- **OVRL**: the overall impression, the closest single number to "is this better?".

DNSMOS scores audio at 16 kHz, so it cannot hear anything above 8 kHz.

### MS-SNSD

[MS-SNSD](https://github.com/microsoft/MS-SNSD): clean speech mixed with real recorded noise,
so the output can be compared to the original clean voice. The script picks 8 clips for each
of 9 noise types and mixes each at 0, 5, 10 and 20 dB SNR: 288 mixtures in total, with a fixed
random seed (7). The speech is 16 kHz, so this set says nothing about the band above 8 kHz.

| | PESQ | ESTOI | NISQA | DNSMOS OVRL | Word errors | Word starts cut |
|---|---|---|---|---|---|---|
| Untouched | 1.59 | 0.785 | 2.58 | 2.33 | 2.4% | 0.0% |
| Voice | 2.01 | 0.844 | 2.88 | 3.09 | 6.6% | 13.0% |
| **Voice HD** | **2.34** | **0.887** | **4.53** | **3.23** | 3.2% | 12.0% |

Word errors are against Whisper's transcript of the clean speech, since MS-SNSD has no
transcripts. Mixed this loud (half the clips at 0 or 5 dB SNR), both modes cut word starts,
mostly at 0 dB (19-20%) and least at 20 dB (7%). Voice again doubles the word errors; Voice HD
adds 0.8 points, which is just significant. Turning the gate off changes the scores by less
than their margin of error, apart from 1-1.5 points fewer word starts cut.

### Noise with no speech

What the listener hears between sentences, which none of the scores above measure: 69 clips of
MS-SNSD test noise (3 per noise type) with no speech in them, at -50, -35 and -20 dBFS. The
first second is skipped while the gate settles.

| | Left over | Noise let through | Level wobble |
|---|---|---|---|
| Untouched | -37 dBFS | 100% of the time | 5.5 dB |
| Filter and gate only, no model | -47 dBFS | 53% | 11.2 dB |
| Voice | -71 dBFS | 8% | 8.1 dB |
| Voice, no gate | -64 dBFS | 9% | 9.2 dB |
| **Voice HD** | **-80 dBFS** | 9% | **5.7 dB** |
| Voice HD, no gate | -77 dBFS | 10% | 6.4 dB |

- **Noise let through**: the share of 10 ms frames turned down by less than 6 dB.
- **Level wobble**: how much the leftover level jumps around, measured as the spread of its 50 ms
  levels. Above the input's own spread, this is pumping.

This is where the gate earns its place: it takes another 2-6 dB off the leftover noise, most of
all for quiet noise (12 dB more at -50 dBFS with Voice). Voice HD removes clicks and typing
almost entirely (-96 dBFS). The one noise type neither model touches is "Neighbor" (speech
through a wall), which is speech, so both let half of it through.

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

For live audio the slowest frames matter more than the average: each 10 ms frame has to be
ready within 10 ms. The models alone, over speech (`cargo run --release -p vidar-denoise
--example bench`, same machine, 3 passes):

| | Median frame | p99 | p99.9 | Slowest | Frames over 10 ms |
|---|---|---|---|---|---|
| Voice | 0.26 ms | 0.30 ms | 0.37 ms | 1.8 ms | 0 |
| Voice HD | 1.14 ms | 1.27 ms | 1.61 ms | 1.6 ms | 0 |
| Voice HD, spectral detector | 0.88 ms | 1.05 ms | 1.22 ms | 1.4 ms | 0 |

Even the slowest frame uses less than a fifth of the budget.

## Reading the numbers

- **Voice HD is better everywhere**, and it is the only mode that removes noise without
  costing words. It has twice the delay (60 vs 30 ms) and uses about five times the processor.
- **Voice is cheap but not free.** It still raises every "how does it sound" score, but it
  doubles word errors and cuts the start of about one word in seven, at about 2.5% of one core.
- **The gate is for the pauses**, not for the speech. It barely changes the scores while
  someone talks, and for Voice HD it costs a little reference quality, but it pushes the leftover
  noise between sentences further down.
- These are averages. Your mic and your room decide what sounds best, which is why the app has
  the Test tab.

## Running it yourself

You need a release build, the data sets and, for the neural scores and Whisper, a PyTorch
install (they run on the GPU when there is one).

```shell
cargo build --release -p vidar-cli
python -m venv .venv && .venv\Scripts\activate
pip install -r bench/requirements.txt
pip install torch torchaudio --index-url https://download.pytorch.org/whl/cu128   # or the CPU build
pip install -r bench/requirements-gpu.txt

# Data sets
python bench/fetch.py vbd --dest ../bench-data/VBD          # 325 MB
python bench/fetch.py dns5 --dest ../bench-data/DNS5        # 3.9 GB
python bench/fetch.py ears-wham --dest ../bench-data/EARS   # 11 GB, see below
# MS-SNSD: clone github.com/microsoft/MS-SNSD
# DNS 2020 blind set and DNSMOS models: github.com/microsoft/DNS-Challenge,
#   datasets/blind_test_set (interspeech2020/master) and DNSMOS/DNSMOS (master)

# With a clean reference
python bench/benchmark.py vbd  --vbd-dir ../bench-data/VBD --dnsmos DNSMOS/DNSMOS --work bench-out
python bench/benchmark.py ears --ears-dir ../bench-data/EARS/EARS-WHAM_v2 --dnsmos DNSMOS/DNSMOS --work bench-out
python bench/benchmark.py snsd --snsd-dir MS-SNSD --dnsmos DNSMOS/DNSMOS --work bench-out
python bench/benchmark.py pairs --clean-dir clean --noisy-dir noisy --name myset --work bench-out

# Real recordings, no reference
python bench/benchmark.py blind --blind-dir blind_test_set --dnsmos DNSMOS/DNSMOS --work bench-out
python bench/benchmark.py blind --blind-dir ../bench-data/DNS5/V5_BlindTestSet_mono --name dns5 --dnsmos DNSMOS/DNSMOS --work bench-out

# Noise with no speech
python bench/benchmark.py noise --noise-dir MS-SNSD/noise_test --systems voice,voice_hd,no_model --work bench-out
```

Options shared by all of them:

- `--systems` picks the configurations to score (default `voice,voice_hd`). See the table below.
- `--metrics` picks the score families: `ref` (PESQ, STOI, ESTOI, SI-SDR, log-spectral distance,
  word start and end loss), `dnsmos`, `nn` (NISQA and SQUIM) and `wer` (Whisper). By default
  it runs every family whose dependencies are installed (`dnsmos` needs `--dnsmos`).
- `--baseline` is the configuration the others are tested against (default: the untouched input).
- `--utmos` adds UTMOS. It is off by default because it loads through `torch.hub`, which runs
  code from the [SpeechMOS](https://github.com/tarepan/SpeechMOS) repository (pinned to v1.2.0).
- `--limit N` scores only the first N clips, for a quick check.

| System | `vidar-cli` flags |
|---|---|
| `voice` | `--engine rnnoise` |
| `voice_hd` | `--engine dpdfnet --dpdfnet-vad rnnoise` |
| `voice_hd_spectral` | `--engine dpdfnet --dpdfnet-vad spectral` |
| `voice_no_gate` | `--engine rnnoise --no-gate` |
| `voice_hd_no_gate` | `--engine dpdfnet --dpdfnet-vad rnnoise --no-gate` |
| `voice_hd_no_declick` | `--engine dpdfnet --dpdfnet-vad rnnoise --no-declick` |
| `voice_hd_gate_out` | `--engine dpdfnet --dpdfnet-vad rnnoise --gate-level output` |
| `voice_hd_8` | `--engine dpdfnet --dpdfnet-vad rnnoise --dpdfnet-model dpdfnet8_48khz_hr` |
| `no_model` | `--no-rnnoise` (filter and gate only) |

The script prints each score with its confidence interval and its difference from the baseline,
then plain averages by SNR, noise type, speaker or recording type. It writes every per-clip
score to `bench-out/<set>_scores.csv` and the intervals to `bench-out/<set>_summary.csv`.
Processed audio is cached (a configuration is only re-run if any of its output files are
missing), and so are the scores, in `bench-out/<set>/scores`. Delete a folder there to score
again after changing a metric.

For the sets with a reference, `vidar-cli` trims its own delay so the output lines up with the
reference. The script checks this by reporting the median alignment lag, which should be 0.

**EARS-WHAM** needs only the six EARS test speakers and the 160 WHAM! test noise files. The
fetch script reads those out of the 73 GB WHAM! archive with HTTP range requests, then runs the
official generator from [ears_benchmark](https://github.com/sp-uhh/ears_benchmark) (pinned
commit) for the test split only. That last step runs code downloaded from GitHub.

The processor figures need no data set: loop any 48 kHz recording to two minutes, then time
`vidar-cli --clean clip.wav out.wav -j 1` with each configuration's flags from the table above.

### Listening test

The scores above are all predictions of what people would say. To ask people, export a
[MUSHRA](https://www.itu.int/rec/R-REC-BS.1534) test for
[webMUSHRA](https://github.com/audiolabs/webMUSHRA) from any scored set with a reference:

```shell
python bench/benchmark.py mushra --work bench-out --name vbd --systems voice,voice_hd --trials 12 --out mushra
```

It picks 3-10 second clips, matches their loudness (-26 LUFS) and writes a config with a hidden
reference and a 3.5 kHz low-pass anchor. Copy `mushra/configs` into webMUSHRA and open
`?config=vidar_vbd.yaml`. Afterwards,
`python bench/benchmark.py mushra-results --csv results/vidar_vbd/mushra.csv` drops listeners
who fail to spot the hidden reference (ITU-R BS.1534-3) and prints each version's average
with its confidence interval.

## Not measured yet

- **DNS Challenge 5 blind set** (2023): 753 real 48 kHz recordings, headset and speakerphone.
  The data set is downloaded; it is one `blind` run.
- **EARS-WHAM**: 48 kHz studio speech in real noise, including whispering, shouting and
  emotional speech that VoiceBank lacks.
- **A listening test.** The export is ready; it needs listeners.
- **Spectral speech detection for Voice HD** (`voice_hd_spectral`). If it scores as well as
  the RNNoise detector, Voice HD can stop running RNNoise alongside DPDFNet.
- **The de-clicker after DPDFNet** (`voice_hd_no_declick`) and **the larger model**
  (`voice_hd_8`) on the new scores. Both have been run on MS-SNSD PESQ and STOI only.
- **Processor use on a laptop CPU**: the figures above are from a fast desktop, and on a
  processor without AVX2 RNNoise falls back to its slower SSE4.1 or plain layers.
- **Delay**, measured end to end in the live app instead of the figures derived from the models.
