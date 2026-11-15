<p align="center">
  <img src="assets/vidar-icon.png" alt="" width="120" />
</p>

<h1 align="center">Vidar</h1>

<p align="center"><b>Real-time AI noise cancellation for Windows.</b><br/>
Offline, on your own CPU. No account, no cloud.</p>

---

Vidar removes fans, keyboards, room noise and background voices from your microphone before
any app hears it. The cleaned voice appears in Windows as a new microphone, **Vidar AI
Microphone**, which Discord, Teams, Zoom, OBS and games use like any other.

```
Your mic → Vidar (filter → AI model → cleanup) → Vidar AI Microphone → your apps
```

## Getting started

1. **Install the virtual microphone.** On first start Vidar offers to install it. Windows asks
   for permission once, and in rare cases wants a restart. Your speakers and microphone stay
   as they are.
2. **Turn Vidar on.** It makes Vidar AI Microphone the Windows default microphone and
   remembers yours.
3. **Check your apps.** Apps set to *Default* switch over by themselves. If an app uses a
   specific device, choose **Vidar AI Microphone** there once.
4. **Turn it off when you like.** When you turn Vidar off, quit or shut down, your own
   microphone comes back. After a crash this happens the next time Vidar starts.

## Two new devices in Windows

Installing the virtual microphone adds **two** devices to Windows' sound settings:

| Device | Where it appears | What it is |
|---|---|---|
| **Vidar AI Microphone** | Input (recording) devices | Your clean voice. Choose this in your apps. |
| **Vidar Internal** | Output (playback) devices | Vidar's private connection to the microphone. Leave it alone. |

**Why is there an output device?** Windows only accepts a virtual microphone in the form of a
kernel-mode audio driver that Microsoft has signed. Getting a driver of its own signed needs
an Extended Validation (EV) code-signing certificate, a Microsoft hardware developer account
and a Microsoft review for every update. That is out of reach for a free, open-source project.
Vidar therefore builds on [VB-Audio Virtual Cable](https://vb-audio.com/Cable/), a widely
used driver that Microsoft has already signed. The cable always comes as a pair: whatever is
played into its output device comes out of its microphone. Vidar plays your cleaned voice into
the output. It renames the two ends from the driver's confusing stock names (*CABLE Input* is
the output, *CABLE Output* the microphone) to names that say what each one is for.

- **Don't choose Vidar Internal as your speakers or headphones.** You would hear nothing, and
  every sound played there would be mixed into the microphone the other side hears.
- **Don't disable it.** Without it no audio reaches Vidar AI Microphone.
- Vidar never makes it your default output. If the driver installer does, Vidar switches back
  to your real speakers straight away.

**Settings → Virtual microphone → Uninstall** gives your own microphone back to Windows and
removes both devices.

## Modes

Two modes, from most to least noise removal:

**1. Voice HD: the cleanest.** A larger AI model (DPDFNet) separates your voice from
everything else, even while you talk. It handles loud fans, traffic, echoing rooms and people
talking nearby best, and your voice sounds the most natural. The price is about 60 ms of delay
and roughly five times the processor work of Voice. Use it for calls, meetings and recordings.

**2. Voice: nearly as clean, much lighter.** A small AI model (RNNoise) does the same job.
In loud or echoing rooms a little more noise stays under your voice. It has about 30 ms of
delay and barely touches the processor. Use it for gaming, on laptops, or when you hear
yourself back. This is the default.

| | Voice HD | Voice |
|---|---|---|
| Removes noise while you talk | ✓ best | ✓ |
| Quiet between phrases | ✓ | ✓ |
| Delay | ~60 ms | ~30 ms |
| Processor use, Task Manager (16-thread desktop) | ~0.7% | ~0.15% |
| Processor use, one core | ~11% | ~2.5% |
| Overall quality (DNSMOS, untouched = 2.22) | **2.99** | 2.81 |

*Processor use* is the share of the computer's power Vidar takes while it runs. Task Manager
shows it as a percentage of the whole processor. Each model runs on a single core, so the
per-core figure shows how much headroom is left: at 11%, Voice HD finishes each 10 ms of audio
in about 1.1 ms. These figures come from an AMD Ryzen 7 9800X3D. An older or low-power laptop
CPU can need two to three times as much, which still leaves Voice HD well within real time.

Both models are trained on speech, so they are not meant for instruments or singing.

**Strength** (Gentle, Balanced or Strong) sets how sure Vidar must be that you are talking
before your microphone turns back up. Not sure which to pick? The **Test** tab records you with
your usual background until you press Stop (up to 20 seconds) and runs that clip through every
mode and strength. Click a row to hear it, drag on the waveform to loop one part while you
switch, and press **Use** on the one that sounds best.

## Quality

The scores above come from Microsoft's DNS Challenge blind test set (600 real noisy
recordings). For results on a second test set, the method and how to reproduce them, see
**[docs/benchmarks.md](docs/benchmarks.md)**.

## Cleaning files

The same processing is available for recordings:

```
vidar-cli --clean interview.m4a                       # writes interview_cleaned.wav
vidar-cli --clean podcast/ podcast-clean/ --engine dpdfnet
```

## Documentation

- [How it works](docs/how-it-works.md): the signal chain step by step, speech detection, the
  virtual cable, routing, clock drift, delay
- [Benchmarks](docs/benchmarks.md): data sets, metrics, results, open questions
- [Development](docs/development.md): building, project layout, tests, command line, settings
  file

## Credits

- [RNNoise](https://github.com/xiph/rnnoise): Jean-Marc Valin, Xiph.Org, Mozilla, Amazon (BSD-3-Clause)
- [DPDFNet](https://huggingface.co/Ceva-IP/DPDFNet): Ceva (Apache-2.0)
- [ONNX Runtime](https://github.com/microsoft/onnxruntime): Microsoft (MIT)
- [VB-Audio Virtual Cable](https://vb-audio.com/Cable/): Vincent Burel, donationware, bundled
  unmodified
- [Inter](https://rsms.me/inter/): Rasmus Andersson (SIL Open Font License)

Vidar is MIT-licensed; see [LICENSE](LICENSE).
