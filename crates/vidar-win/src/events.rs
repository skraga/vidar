//! Device change notifications.

use std::sync::mpsc::Sender;

use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl,
    DEVICE_STATE,
};

use crate::model::Flow;
use crate::sys::WinAudio;
use crate::Error;

/// What changed. IDs are endpoint IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Added(String),
    Removed(String),
    StateChanged {
        id: String,
        state: u32,
    },
    /// Console-role default changed (other roles are ignored to avoid triple events).
    DefaultChanged {
        flow: Flow,
        id: Option<String>,
    },
}

#[implement(IMMNotificationClient)]
struct Client {
    tx: Sender<DeviceEvent>,
}

fn s(p: &PCWSTR) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: Windows passes a valid NUL-terminated string for the duration of the call.
        unsafe { p.to_string().ok() }
    }
}

impl IMMNotificationClient_Impl for Client_Impl {
    fn OnDeviceStateChanged(&self, id: &PCWSTR, state: DEVICE_STATE) -> windows::core::Result<()> {
        if let Some(id) = s(id) {
            let _ = self
                .tx
                .send(DeviceEvent::StateChanged { id, state: state.0 });
        }
        Ok(())
    }
    fn OnDeviceAdded(&self, id: &PCWSTR) -> windows::core::Result<()> {
        if let Some(id) = s(id) {
            let _ = self.tx.send(DeviceEvent::Added(id));
        }
        Ok(())
    }
    fn OnDeviceRemoved(&self, id: &PCWSTR) -> windows::core::Result<()> {
        if let Some(id) = s(id) {
            let _ = self.tx.send(DeviceEvent::Removed(id));
        }
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        id: &PCWSTR,
    ) -> windows::core::Result<()> {
        if role == eConsole {
            let flow = if flow == eCapture {
                Flow::Capture
            } else {
                Flow::Render
            };
            let _ = self
                .tx
                .send(DeviceEvent::DefaultChanged { flow, id: s(id) });
        }
        Ok(())
    }
    fn OnPropertyValueChanged(
        &self,
        _id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Keeps the registration alive; unregisters on drop.
pub struct DeviceWatcher {
    audio: WinAudio,
    client: IMMNotificationClient,
}

impl DeviceWatcher {
    /// Events are delivered on a Windows thread; keep the receiver side cheap.
    pub fn start(tx: Sender<DeviceEvent>) -> Result<Self, Error> {
        let audio = WinAudio::new()?;
        let client: IMMNotificationClient = Client { tx }.into();
        // SAFETY: the client stays alive in `self` until unregistered in Drop.
        unsafe {
            audio
                .enumerator()
                .RegisterEndpointNotificationCallback(&client)
        }
        .map_err(|e| Error::Com(e.to_string()))?;
        Ok(Self { audio, client })
    }
}

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        // SAFETY: same client instance that was registered.
        unsafe {
            let _ = self
                .audio
                .enumerator()
                .UnregisterEndpointNotificationCallback(&self.client);
        }
    }
}
