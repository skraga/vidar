# Changelog

What changed in each Vidar release, newest first. Sections are generated from commit
messages by [git-cliff](https://git-cliff.org) when a release is prepared
(`cargo xtask release <version>`), and each one is published as that version's GitHub
release notes.

## 0.3.1 — 2026-11-20

### Fixed

- Download files name their architecture the same way: x64 or arm64

## 0.3.0 — 2026-11-20

### New

- Native version for Windows 11 on ARM (Snapdragon PCs): `vidar-0.3.0-arm64-setup.exe`
- Set your microphone's level in Windows from Settings → Microphone, with a warning when the
  microphone is muted in Windows or overloading
- Setup installs the Microsoft Visual C++ Runtime when it is missing

### Fixed

- Changing the voice tone no longer clicks, and switching it back on no longer pops

## 0.2.0 — 2026-11-19

### New

- **Installer.** `vidar-0.2.0-setup.exe` installs Vidar and, if it is missing, the virtual
  microphone, with a single permission prompt. Vidar gets a Start menu entry, an optional
  desktop icon and an entry in *Installed apps*. Running a newer setup upgrades Vidar in place.
  It first closes the running copy, which switches your own microphone back. Uninstalling asks
  whether to remove the virtual microphone and your settings as well.
- **Updates from inside the app.** About once a day Vidar checks whether a new version is out.
  If one is, Home and Settings tell you. Installed copies update with **Update now**: Vidar
  downloads the setup, checks it against its published checksum, installs it and starts again.
  The portable zip opens the download page instead. Turn the check off in Settings → Updates.
- **Seven-band voice tone.** The equaliser now has 7 bands (was 5): 100 Hz, 200 Hz, 500 Hz,
  1 kHz, 2.5 kHz, 5 kHz and 8 kHz. Warmth, boxiness and harsh *s* sounds can each be set on
  their own.
- **Ten tone presets.** New: Bright, Soft, Less nasal, Gaming, Headset, Laptop and Close mic,
  next to Warm, Clear and Broadcast. Hover a preset to see what it is for.
- **A new look.** Vidar follows Windows' light or dark mode, uses Windows 11-style controls,
  and has a new power button with level bars that move with your voice. Keyboard users see
  where the focus is, and text and controls meet WCAG 2.2 AA contrast in both themes.

### Fixed

- Pressing **Stop** while Vidar was reconnecting to a lost microphone now stops it. Before,
  Vidar started again on its own a second and a half later.
- Unmuting no longer plays back a short burst of what you said just before muting.
- Switching between Voice and Voice HD while running could cause a short glitch in the audio.
- If Vidar could not start sending audio to the virtual microphone, it kept your microphone
  open in the background. It now releases it.
- Mic boost for built-in laptop microphones now always applies to the microphone actually
  in use.
- The version badge on the About page no longer stretches across the whole page. VB-Audio
  now has a single **Support VB-Audio** button.

### Notes

- A custom voice tone saved in 0.1.0 starts flat, because the curve now has seven points.
  Presets and all other settings are kept.
- `vidar-cli.exe` now shows Vidar's icon and version, including in the Windows permission
  prompt when it installs the virtual microphone.

## 0.1.0 — 2026-11-16

First release.

- Real-time noise removal for your microphone, offline on your own CPU. It takes out fans,
  keyboards, room noise and background voices.
- **Vidar AI Microphone**, a microphone Discord, Teams, Zoom, OBS and games can use like any
  other. It is based on VB-Audio's free VB-CABLE driver, installed for you with one
  permission prompt.
- Two modes: **Voice** (RNNoise, about 30 ms delay) and **Voice HD** (DPDFNet, about 60 ms).
- A gate that keeps pauses quiet without cutting off the start or the end of words, with
  Gentle, Balanced and Strong settings.
- Rumble filter, de-clicker for keyboard clicks, optional automatic level, and a five-band
  voice tone with presets.
- Makes Vidar AI Microphone the default microphone while it runs, and switches your own
  microphone back when it stops, even after a crash or a power cut.
- A Test tab to hear yourself before and after cleaning, and `vidar-cli` to clean audio
  files from the command line.
