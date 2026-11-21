<p align="center">
  <img src="assets/vidar-icon.png" alt="" width="120" />
</p>

<h1 align="center">Vidar</h1>

<p align="center"><b>Sound clear on every call, wherever you are.</b><br/>
AI noise cancellation for Windows. Free, open source, and it never leaves your PC.</p>

<p align="center">
  <a href="https://github.com/skraga/vidar/releases/latest"><b>⬇ Download for Windows</b></a>
  &nbsp;·&nbsp; Windows 10 and 11 &nbsp;·&nbsp; Intel, AMD and ARM64
</p>

<p align="center">
  <a href="https://github.com/skraga/vidar/releases/latest"><img src="https://img.shields.io/github/v/release/skraga/vidar?include_prereleases&sort=semver&label=release&color=4c8bf5" alt="Latest release" /></a>
  <a href="https://github.com/skraga/vidar/releases"><img src="https://img.shields.io/github/downloads/skraga/vidar/total?color=4c8bf5" alt="Downloads" /></a>
  <a href="https://github.com/skraga/vidar/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/skraga/vidar/ci.yml?branch=main&label=CI" alt="CI status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="MIT licence" /></a>
</p>

---

The fan behind you, the keyboard under your hands, the street outside, the people talking in
the next room: Vidar takes them out of your microphone before anyone hears them. What's left is
your voice, clear and natural, in **Discord, Teams, Zoom, Meet, Slack, OBS and every game**,
with nothing to set up in those apps.

## What Vidar does

| | |
|---|---|
| 🔇 **Noise Cancellation** | Fans, traffic, crowds and voices behind you, removed even while you talk |
| ⌨️ **Click & Pop Removal** | Keyboard clicks, desk thumps and blown P and B sounds, softened as they happen |
| 🤫 **Quiet Pauses** | Your room fades out between words, without cutting off the first syllable |
| 🎚️ **Auto Equalizer** | Automatically moves a laptop, headset or webcam mic towards a studio sound |
| 🎙️ **Podcast Voice** | One switch for a close, even voice like a podcast host |
| 📢 **Auto Level** | Lifts a quiet mic to a comfortable level, never the background |
| 🎧 **Try Before You Choose** | Record yourself once, hear every mode, keep the best |
| 💻 **Light on Your PC** | Under 1% of the processor in everyday mode, pauses when no app listens |
| 🔒 **Private** | 100% on your PC: no account, no cloud, nothing saved or uploaded |
| 🎛️ **Per-Mic Settings** | Each microphone remembers its own mode and settings |
| 📁 **File Cleaning** | The same processing for podcasts, interviews and voice-overs |

```
vidar-cli --clean interview.m4a                      # writes interview_cleaned.wav
vidar-cli --clean take.wav --engine dpdfnet8 --studio # strongest mode, with Podcast voice
```

## Three modes, one click

| | **Voice HD+** | **Voice HD** (default) | **Voice** |
|---|---|---|---|
| Best for | The loudest noise: babble, traffic, big fans | Calls, meetings, recordings | Gaming, laptops, hearing yourself back |
| Removes noise while you talk | ✓ | ✓ | ✓ |
| Delay | ~60 ms | ~60 ms | ~30 ms |
| Processor use (Task Manager, 16-thread desktop) | ~1.7% | ~0.7% | ~0.15% |

Not sure? The **Sound** tab lets you hear all three on your own voice and your own room.
Both AI models are trained on speech, so Vidar is made for voices, not for instruments or
singing.

## Proven quality

On Microsoft's DNS Challenge blind test set (600 real noisy recordings), Voice HD raises the
overall quality score (DNSMOS) from **2.22** for the untouched recordings to **2.99**. Voice
reaches **2.81**. On VoiceBank+DEMAND, Voice HD+ scores level with Voice HD. Method, more test sets and how to
reproduce the results: **[docs/benchmarks.md](docs/benchmarks.md)**.

## Get started in a minute

1. **[Download](https://github.com/skraga/vidar/releases/latest)** the setup for your PC
   (`x64` for Intel and AMD, `arm64` for Snapdragon) and run it.
2. **Turn Vidar on.** It becomes your Windows microphone, **Vidar AI Microphone**.
3. **Talk.** Apps set to the default microphone switch over by themselves. Turn Vidar off and
   your own microphone comes straight back.

Vidar updates itself from inside the app. A portable zip is available too. For both, plus
why Windows shows a second device called *Vidar Internal*, and how to uninstall, see
**[docs/install.md](docs/install.md)**.

## Documentation

- [Install, update and uninstall](docs/install.md): downloads, the two Windows devices,
  updates, removing Vidar, building your own package
- [How it works](docs/how-it-works.md): the signal chain step by step, speech detection, the
  virtual cable, routing, clock drift, delay
- [Benchmarks](docs/benchmarks.md): data sets, metrics, results, open questions
- [Development](docs/development.md): building, project layout, tests, command line, settings
  file

## Credits

- [RNNoise](https://github.com/xiph/rnnoise): Jean-Marc Valin, Xiph.Org, Mozilla, Amazon (BSD-3-Clause)
- [DPDFNet](https://huggingface.co/Ceva-IP/DPDFNet): Ceva (Apache-2.0)
- [ONNX Runtime](https://github.com/microsoft/onnxruntime): Microsoft (MIT)
- [VB-Audio Virtual Cable](https://www.vb-cable.com): VB-Audio Software (Vincent Burel), bundled
  unmodified. VB-CABLE is donationware, all participations are welcome:
  [support VB-Audio](https://vb-audio.com/Cable/).
- [Inter](https://rsms.me/inter/): Rasmus Andersson (SIL Open Font License)

Vidar's own code is MIT-licensed; see [LICENSE](LICENSE). Third-party components keep their own
licences, listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). In particular,
`drivers/vbcable/` is VB-CABLE by VB-Audio Software: proprietary donationware, not covered by
the MIT licence, and redistributed unmodified under
[VB-Audio's terms](https://vb-audio.com/Services/licensing.htm).
