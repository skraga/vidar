//! Thin WASAPI shared-mode stream wrapper (event driven, float32, 48 kHz).

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, AudioCategory_Other, AudioClientProperties, IAudioCaptureClient,
    IAudioClient, IAudioClient2, IAudioClient3, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDCLNT_STREAMOPTIONS_RAW, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
    WaitForSingleObject,
};

use vidar_win::Flow;

pub const SAMPLE_RATE: u32 = 48_000;

/// A Windows error with its HRESULT kept for [`crate::explain`].
#[derive(Debug, Clone)]
pub struct WinError {
    pub code: u32,
    pub message: String,
}

impl From<windows::core::Error> for WinError {
    fn from(e: windows::core::Error) -> Self {
        Self {
            code: e.code().0 as u32,
            message: e.message().to_string(),
        }
    }
}

impl std::fmt::Display for WinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:08X} {}", self.code, self.message)
    }
}

pub type WinResult<T> = Result<T, WinError>;

#[derive(Debug, Clone, Copy)]
pub struct StreamOptions {
    /// Requested buffer in milliseconds (bigger for Bluetooth hands-free).
    pub buffer_ms: u32,
    /// Ask for the signal without vendor processing (capture only).
    pub raw: bool,
    /// Try the device's smallest shared-mode period (IAudioClient3).
    pub low_latency: bool,
}

/// An opened, not yet started stream.
pub struct Stream {
    pub client: IAudioClient,
    pub event: HANDLE,
    pub channels: usize,
    pub buffer_frames: u32,
    pub low_latency_period: Option<u32>,
    pub device_name: String,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stop is harmless if not started; the event handle was created by us.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

fn float_format(channels: u16) -> WAVEFORMATEXTENSIBLE {
    let block = channels * 4;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                - std::mem::size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: 32,
        },
        dwChannelMask: match channels {
            1 => 0x4,
            _ => 0x3,
        },
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

/// Is the mix format already 48 kHz float32? Then IAudioClient3 can run at the engine's
/// own period without conversion.
unsafe fn mix_is_48k_float(mix: *const WAVEFORMATEX) -> bool {
    let m = &*mix;
    if m.nSamplesPerSec != SAMPLE_RATE || m.wBitsPerSample != 32 {
        return false;
    }
    if m.wFormatTag == WAVE_FORMAT_EXTENSIBLE as u16 {
        let ext = mix as *const WAVEFORMATEXTENSIBLE;
        // The struct is packed; read the GUID without taking a reference.
        std::ptr::addr_of!((*ext).SubFormat).read_unaligned() == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    } else {
        m.wFormatTag == 3 // WAVE_FORMAT_IEEE_FLOAT
    }
}

pub fn enumerator() -> WinResult<IMMDeviceEnumerator> {
    vidar_win::sys::com_init();
    // SAFETY: standard COM creation.
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }?)
}

/// Device by endpoint ID, or the console default for the flow.
pub fn device(en: &IMMDeviceEnumerator, id: Option<&str>, flow: Flow) -> WinResult<IMMDevice> {
    // SAFETY: COM lookups.
    unsafe {
        Ok(match id {
            Some(id) => en.GetDevice(PCWSTR(HSTRING::from(id).as_ptr()))?,
            None => en.GetDefaultAudioEndpoint(
                if flow == Flow::Capture {
                    eCapture
                } else {
                    eRender
                },
                eConsole,
            )?,
        })
    }
}

/// Open a shared-mode, event-driven float32 48 kHz stream with the device's channel count
/// (1 or 2; more are opened as 2 and Windows downmixes).
pub fn open(dev: &IMMDevice, flow: Flow, opts: StreamOptions) -> WinResult<Stream> {
    let device_name = vidar_win::WinAudio::new()
        .ok()
        .and_then(|a| {
            // SAFETY: ID string read from a valid device and freed after use.
            let id = unsafe {
                dev.GetId().ok().map(|p| {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                })
            }?;
            use vidar_win::AudioSystem;
            a.endpoint(&id).map(|e| e.name)
        })
        .unwrap_or_default();

    // SAFETY: COM calls on a valid device; the mix format is freed with CoTaskMemFree.
    unsafe {
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;

        if opts.raw && flow == Flow::Capture {
            if let Ok(c2) = client.cast::<IAudioClient2>() {
                let props = AudioClientProperties {
                    cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                    bIsOffload: false.into(),
                    eCategory: AudioCategory_Other,
                    Options: AUDCLNT_STREAMOPTIONS_RAW,
                };
                // Devices without raw support just ignore this.
                let _ = c2.SetClientProperties(&props);
            }
        }

        let mix = client.GetMixFormat()?;
        let mix_channels = (*mix).nChannels.clamp(1, 2);
        let mix_native = mix_is_48k_float(mix);

        let event = CreateEventW(None, false, false, None)?;
        let mut low_latency_period = None;

        // Low-latency path: needs the exact mix format (no conversion) at 48 kHz float.
        if opts.low_latency && mix_native && (*mix).nChannels <= 2 {
            if let Ok(c3) = client.cast::<IAudioClient3>() {
                let (mut def, mut fund, mut min, mut max) = (0u32, 0u32, 0u32, 0u32);
                if c3
                    .GetSharedModeEnginePeriod(mix, &mut def, &mut fund, &mut min, &mut max)
                    .is_ok()
                    && min > 0
                    && min < def
                    && c3
                        .InitializeSharedAudioStream(
                            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                            min,
                            mix,
                            None,
                        )
                        .is_ok()
                {
                    low_latency_period = Some(min);
                }
            }
        }
        let channels = if low_latency_period.is_some() {
            (*mix).nChannels
        } else {
            mix_channels
        };
        CoTaskMemFree(Some(mix as *const _));

        if low_latency_period.is_none() {
            let fmt = float_format(channels);
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            let hns = i64::from(opts.buffer_ms) * 10_000;
            if let Err(e) =
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, hns, 0, &fmt.Format, None)
            {
                let _ = CloseHandle(event);
                return Err(e.into());
            }
        }

        client.SetEventHandle(event)?;
        let buffer_frames = client.GetBufferSize()?;
        Ok(Stream {
            client,
            event,
            channels: usize::from(channels),
            buffer_frames,
            low_latency_period,
            device_name,
        })
    }
}

impl Stream {
    pub fn capture_client(&self) -> WinResult<IAudioCaptureClient> {
        // SAFETY: initialised capture stream.
        Ok(unsafe { self.client.GetService()? })
    }
    pub fn render_client(&self) -> WinResult<IAudioRenderClient> {
        // SAFETY: initialised render stream.
        Ok(unsafe { self.client.GetService()? })
    }
    pub fn start(&self) -> WinResult<()> {
        // SAFETY: initialised stream.
        unsafe { self.client.Start()? };
        Ok(())
    }
    /// Wait for the next buffer event. False on timeout.
    pub fn wait(&self, timeout_ms: u32) -> bool {
        // SAFETY: valid event handle owned by the stream.
        unsafe { WaitForSingleObject(self.event, timeout_ms) == WAIT_OBJECT_0 }
    }
}

/// Raises the calling thread to the "Pro Audio" scheduling class; reverts on drop.
pub struct MmcssGuard(HANDLE);

impl MmcssGuard {
    pub fn pro_audio() -> Option<Self> {
        let mut index = 0u32;
        // SAFETY: registers this thread with MMCSS; reverted in Drop.
        unsafe {
            AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut index)
                .ok()
                .map(MmcssGuard)
        }
    }
}

impl Drop for MmcssGuard {
    fn drop(&mut self) {
        // SAFETY: handle from AvSetMmThreadCharacteristicsW.
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}
