<p align="center">
  <img src="assets/vidar-icon.png" alt="" width="120" />
</p>

<h1 align="center">Vidar</h1>

<p align="center"><b>Real-time AI noise cancellation for Windows.</b><br/>
Offline, on your own CPU. No account, no cloud.</p>

<p align="center">
  <a href="https://github.com/skraga/vidar/releases/latest"><img src="https://img.shields.io/github/v/release/skraga/vidar?include_prereleases&sort=semver&label=release&color=4c8bf5" alt="Latest release" /></a>
  <a href="https://github.com/skraga/vidar/releases"><img src="https://img.shields.io/github/downloads/skraga/vidar/total?color=4c8bf5" alt="Downloads" /></a>
  <a href="https://github.com/skraga/vidar/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/skraga/vidar/ci.yml?branch=main&label=CI" alt="CI status" /></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows&logoColor=white" alt="Windows 10 | 11" />
  <img src="https://img.shields.io/badge/Rust-1.95%2B-CE422B?logo=rust&logoColor=white" alt="Rust 1.95+" />
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="MIT licence" /></a>
</p>

---

Vidar removes fans, keyboards, room noise and background voices from your microphone before
any app hears it. The cleaned voice appears in Windows as a new microphone, **Vidar AI
Microphone**, which Discord, Teams, Zoom, OBS and games use like any other.

```
Your mic → Vidar (filter → AI model → cleanup) → Vidar AI Microphone → your apps
```

## Install

Vidar needs Windows 10 (version 1903 or later) or Windows 11 on a 64-bit PC: Intel or AMD
(x64), or ARM64 such as Snapdragon (Windows 11 only). From the [Releases](../../releases)
page, download either:

| | Intel / AMD | ARM64 |
|---|---|---|
| **Setup** (recommended) | `vidar-<version>-setup.exe` | `vidar-<version>-arm64-setup.exe` |
| **Portable** | `vidar-<version>-windows-x64.zip` | `vidar-<version>-windows-arm64.zip` |

- **Setup** installs Vidar and the virtual microphone with one permission prompt, plus the
  Microsoft Visual C++ Runtime if your PC does not have it yet (downloaded from Microsoft).
  It adds Vidar to the Start menu and to *Installed apps*, and later versions install from
  inside the app.
- **Portable**: extract it anywhere and run `vidar.exe`. On first start it offers to install
  the virtual microphone. It does not update itself; Vidar tells you when a new version is
  out and opens the download page. Voice HD needs the
  [Microsoft Visual C++ Runtime](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist);
  without it Vidar uses Voice.

To build it from source instead, see [docs/development.md](docs/development.md#building). To get
the same folder you can copy to another PC or zip up, run:

```
cargo xtask dist         # dist\vidar
cargo xtask installer    # dist\vidar, then dist\vidar-<version>-setup.exe
```

This fetches the models and ONNX Runtime, builds the release binaries and puts everything Vidar
needs in `dist\vidar`, licence files included. It needs
[cargo-about](https://github.com/EmbarkStudios/cargo-about) for the Rust crate licences
(`cargo install cargo-about --locked --features cli`), and the installer needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

```
vidar.exe, vidar-cli.exe, onnxruntime.dll
models\dpdfnet2_48khz_hr.onnx   Voice HD (dist --all adds dpdfnet8)
drivers\vbcable\                the virtual microphone (VB-CABLE by VB-Audio)
LICENSE, THIRD_PARTY_NOTICES.md, licenses\
```

Keep the licence files with the program when you share it. Without `onnxruntime.dll` or the model, Voice HD falls back to Voice.

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
This is the default.

**2. Voice: nearly as clean, much lighter.** A small AI model (RNNoise) does the same job.
In loud or echoing rooms a little more noise stays under your voice. It has about 30 ms of
delay and barely touches the processor. Use it for gaming, on laptops, or when you hear
yourself back.

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

The same processing is available for recordings, with `vidar-cli.exe`, which is built and
shipped next to `vidar.exe`:

```
vidar-cli --clean interview.m4a                       # writes interview_cleaned.wav
vidar-cli --clean podcast/ podcast-clean/ --engine rnnoise   # Voice instead of Voice HD
```

## Updates

About once a day Vidar asks GitHub for the latest version number. That is the only network
access it makes, and no personal data is sent. When a newer version is out, Home and
**Settings → Updates** say so:

- **Installed with setup:** **Update now** downloads the new setup, checks it against its
  published SHA-256, and runs it. Windows asks for permission once; Vidar gives your own
  microphone back, updates and starts again.
- **Portable zip:** **Download** opens the release page.

Turn off **Check for updates** in Settings to stop the daily check; **Check now** still works.

## Uninstall

**Installed with setup:** remove Vidar in **Settings → Apps → Installed apps**. It quits
Vidar (your own microphone comes back), turns off *Start with Windows*, and asks whether to
remove the virtual microphone and whether to delete your settings.

**Portable zip:** removing it takes a few steps. Do them in this order: the first one needs
Vidar itself.

1. **Remove the virtual microphone.** In Vidar, open **Settings → Apps and Windows → Virtual
   microphone → Uninstall** and confirm. Vidar turns off, makes your own microphone the Windows
   default again, restores the cable's original names and icons, and runs VB-Audio's setup to
   **delete the VB-CABLE driver** together with both devices (Vidar AI Microphone and Vidar
   Internal). Windows asks for permission and may want a restart. Apps that were set to Vidar
   AI Microphone switch to another microphone.
2. **Turn off "Start with Windows"** in the same section if it is on. Otherwise Windows keeps
   trying to start the deleted program at sign-in.
3. **Quit Vidar** from its tray icon (right-click → Exit) and delete the Vidar folder.
4. **Optional: delete the leftover settings.** `%APPDATA%\Vidar` holds your settings
   (`config.json`, or a `config.json` in the Vidar folder in portable mode), and
   `%ProgramData%\Vidar` holds the backup of the cable's original device names.

**Already deleted the Vidar folder?** The driver stays installed until you remove it. Download
the VB-CABLE package from [www.vb-cable.com](https://www.vb-cable.com), right-click
`VBCABLE_Setup_x64.exe`, choose **Run as administrator**, click **Remove Driver** and restart
Windows. If another program also uses VB-CABLE, removing the driver affects that program too.

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
- [VB-Audio Virtual Cable](https://www.vb-cable.com): VB-Audio Software (Vincent Burel), bundled
  unmodified. VB-CABLE is donationware, all participations are welcome:
  [support VB-Audio](https://vb-audio.com/Cable/).
- [Inter](https://rsms.me/inter/): Rasmus Andersson (SIL Open Font License)

Vidar's own code is MIT-licensed; see [LICENSE](LICENSE). Third-party components keep their own
licences, listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). In particular,
`drivers/vbcable/` is VB-CABLE by VB-Audio Software: proprietary donationware, not covered by
the MIT licence, and redistributed unmodified under
[VB-Audio's terms](https://vb-audio.com/Services/licensing.htm).
