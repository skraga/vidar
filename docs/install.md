# Install, update and uninstall

## Requirements

Windows 10 (version 1903 or later) or Windows 11 on a 64-bit PC: Intel or AMD (amd64), or ARM64
such as Snapdragon (Windows 11 only).

## Download

From the [Releases](https://github.com/skraga/vidar/releases/latest) page, download either:

| | Intel / AMD | ARM64 |
|---|---|---|
| **Setup** (recommended) | `vidar-<version>-amd64-setup.exe` | `vidar-<version>-arm64-setup.exe` |
| **Portable** | `vidar-<version>-amd64-portable.zip` | `vidar-<version>-arm64-portable.zip` |

- **Setup** installs Vidar and the virtual microphone with one permission prompt, plus the
  Microsoft Visual C++ Runtime if your PC does not have it yet (downloaded from Microsoft).
  It adds Vidar to the Start menu and to *Installed apps*, and later versions install from
  inside the app.
- **Portable**: extract it anywhere and run `vidar.exe`. On first start it offers to install
  the virtual microphone. It does not update itself; Vidar tells you when a new version is
  out and opens the download page. Voice HD needs the
  [Microsoft Visual C++ Runtime](https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist);
  without it Vidar uses Voice.

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

## Updates

When it starts and every 8 hours after that, Vidar asks GitHub for the latest version number.
That is the only network access it makes, and no personal data is sent. When a newer version
is out, Home and **About** say so:

- **Installed with setup:** **Update now** downloads the new setup, checks it against its
  published SHA-256, and runs it. Windows asks for permission once; Vidar gives your own
  microphone back, updates and starts again.
- **Portable zip:** **Download** opens the release page.

Turn off **Check for updates** in About to stop the automatic checks; **Check now** still
works.

## Uninstall

**Installed with setup:** remove Vidar in **Settings → Apps → Installed apps**. It quits
Vidar (your own microphone comes back), turns off *Start with Windows*, and asks whether to
remove the virtual microphone and whether to delete your settings.

**Portable zip:** removing it takes a few steps. Do them in this order: the first one needs
Vidar itself.

1. **Remove the virtual microphone.** In Vidar, open **Settings → Virtual
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

## Build your own package

To build Vidar from source instead, see [development.md](development.md#building). To get the
same folder you can copy to another PC or zip up, run:

```
cargo xtask dist         # dist\vidar
cargo xtask installer    # dist\vidar, then dist\vidar-<version>-<amd64|arm64>-setup.exe
```

This fetches the models and ONNX Runtime, builds the release binaries and puts everything Vidar
needs in `dist\vidar`, licence files included. It needs
[cargo-about](https://github.com/EmbarkStudios/cargo-about) for the Rust crate licences
(`cargo install cargo-about --locked --features cli`), and the installer needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

```
vidar.exe, vidar-cli.exe, onnxruntime.dll
models\dpdfnet2_48khz_hr.onnx   Voice HD
models\dpdfnet8_48khz_hr.onnx   Voice HD+
drivers\vbcable\                the virtual microphone (VB-CABLE by VB-Audio)
LICENSE, THIRD_PARTY_NOTICES.md, licenses\
```

Keep the licence files with the program when you share it. Without `onnxruntime.dll` or a
model, Voice HD+ falls back to Voice HD, and Voice HD to Voice.
