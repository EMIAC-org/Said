use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreAction {
    Noop,
    RestoreUnmuted,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSuppression {
    generation: u64,
    restore_action: RestoreAction,
}

#[derive(Debug, Default)]
pub struct SpeakerSuppressionGuard {
    generation: AtomicU64,
    active: Mutex<Option<ActiveSuppression>>,
}

impl SpeakerSuppressionGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&self, reason: &str) {
        if suppression_disabled() {
            tracing::debug!("[speaker_suppression] disabled by env; reason={reason}");
            return;
        }

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(true) = platform::default_output_is_bluetooth() {
            tracing::info!(
                "[speaker_suppression] skipped Bluetooth output; reason={reason} gen={generation}"
            );
            if let Ok(mut active) = self.active.lock() {
                *active = Some(ActiveSuppression {
                    generation,
                    restore_action: RestoreAction::Noop,
                });
            }
            return;
        }

        let restore_action = match platform::current_output_muted() {
            Ok(true) => {
                tracing::debug!(
                    "[speaker_suppression] output already muted; reason={reason} gen={generation}"
                );
                RestoreAction::Noop
            }
            Ok(false) => match platform::set_output_muted(true) {
                Ok(()) => {
                    tracing::info!(
                        "[speaker_suppression] muted Mac output for recording; reason={reason} gen={generation}"
                    );
                    RestoreAction::RestoreUnmuted
                }
                Err(e) => {
                    tracing::warn!("[speaker_suppression] failed to mute output: {e}");
                    RestoreAction::Noop
                }
            },
            Err(e) => {
                tracing::warn!("[speaker_suppression] failed to read output mute state: {e}");
                RestoreAction::Noop
            }
        };

        if let Ok(mut active) = self.active.lock() {
            *active = Some(ActiveSuppression {
                generation,
                restore_action,
            });
        }
    }

    pub fn restore(&self, reason: &str) {
        let active = self.active.lock().ok().and_then(|mut active| active.take());
        let Some(active) = active else {
            return;
        };

        match active.restore_action {
            RestoreAction::Noop => {
                tracing::debug!(
                    "[speaker_suppression] restore noop; reason={reason} gen={}",
                    active.generation
                );
            }
            RestoreAction::RestoreUnmuted => match platform::set_output_muted(false) {
                Ok(()) => tracing::info!(
                    "[speaker_suppression] restored Mac output; reason={reason} gen={}",
                    active.generation
                ),
                Err(e) => tracing::warn!(
                    "[speaker_suppression] failed to restore Mac output; reason={reason} gen={} err={e}",
                    active.generation
                ),
            },
        }
    }
}

impl Drop for SpeakerSuppressionGuard {
    fn drop(&mut self) {
        let active = self.active.get_mut().ok().and_then(Option::take);
        let Some(active) = active else {
            return;
        };
        if active.restore_action == RestoreAction::RestoreUnmuted {
            let _ = platform::set_output_muted(false);
        }
    }
}

fn suppression_disabled() -> bool {
    std::env::var("AIRNOTE_DISABLE_SPEAKER_SUPPRESSION")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            matches!(value.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
mod platform {
    use libc::c_void;

    type AudioObjectID = u32;
    type AudioDeviceID = u32;
    type OSStatus = i32;
    type Boolean = u8;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    const K_AUDIO_OBJECT_SYSTEM_OBJECT: AudioObjectID = 1;
    const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = fourcc(*b"dOut");
    const K_AUDIO_DEVICE_PROPERTY_MUTE: u32 = fourcc(*b"mute");
    const K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(*b"tran");
    const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH: u32 = fourcc(*b"blue");
    const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE: u32 = fourcc(*b"blea");
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
    const K_AUDIO_DEVICE_PROPERTY_SCOPE_OUTPUT: u32 = fourcc(*b"outp");
    const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            io_data_size: *mut u32,
            out_data: *mut c_void,
        ) -> OSStatus;

        fn AudioObjectSetPropertyData(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            in_data_size: u32,
            in_data: *const c_void,
        ) -> OSStatus;

        fn AudioObjectHasProperty(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
        ) -> Boolean;

        fn AudioObjectIsPropertySettable(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            out_is_settable: *mut Boolean,
        ) -> OSStatus;
    }

    pub fn current_output_muted() -> Result<bool, String> {
        let device = default_output_device()?;
        let address = output_mute_address(device)?;
        let mut muted: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut muted as *mut u32).cast::<c_void>(),
            )
        };
        if status == 0 {
            Ok(muted != 0)
        } else {
            Err(format!("AudioObjectGetPropertyData(mute) status={status}"))
        }
    }

    pub fn set_output_muted(muted: bool) -> Result<(), String> {
        let device = default_output_device()?;
        let address = output_mute_address(device)?;
        let mut settable: Boolean = 0;
        let status = unsafe { AudioObjectIsPropertySettable(device, &address, &mut settable) };
        if status != 0 {
            return Err(format!("AudioObjectIsPropertySettable status={status}"));
        }
        if settable == 0 {
            return Err("default output mute property is not settable".into());
        }

        let value: u32 = u32::from(muted);
        let status = unsafe {
            AudioObjectSetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                std::mem::size_of::<u32>() as u32,
                (&value as *const u32).cast::<c_void>(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                "AudioObjectSetPropertyData(mute={muted}) status={status}"
            ))
        }
    }

    pub fn default_output_is_bluetooth() -> Result<bool, String> {
        let transport = default_output_transport_type()?;
        Ok(is_bluetooth_transport(transport))
    }

    pub(super) fn is_bluetooth_transport(transport: u32) -> bool {
        matches!(
            transport,
            K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH | K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE
        )
    }

    fn default_output_device() -> Result<AudioDeviceID, String> {
        let address = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut device: AudioDeviceID = 0;
        let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut device as *mut AudioDeviceID).cast::<c_void>(),
            )
        };
        if status == 0 && device != 0 {
            Ok(device)
        } else {
            Err(format!(
                "AudioObjectGetPropertyData(default output) status={status} device={device}"
            ))
        }
    }

    fn default_output_transport_type() -> Result<u32, String> {
        let device = default_output_device()?;
        let address = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        if unsafe { AudioObjectHasProperty(device, &address) } == 0 {
            return Err("default output device has no transport type property".into());
        }

        let mut transport: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut transport as *mut u32).cast::<c_void>(),
            )
        };
        if status == 0 {
            Ok(transport)
        } else {
            Err(format!(
                "AudioObjectGetPropertyData(transport) status={status}"
            ))
        }
    }

    fn output_mute_address(device: AudioDeviceID) -> Result<AudioObjectPropertyAddress, String> {
        let mut address = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_MUTE,
            m_scope: K_AUDIO_DEVICE_PROPERTY_SCOPE_OUTPUT,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        if unsafe { AudioObjectHasProperty(device, &address) } != 0 {
            return Ok(address);
        }

        address.m_element = 0;
        if unsafe { AudioObjectHasProperty(device, &address) } != 0 {
            return Ok(address);
        }

        Err("default output device has no mute property".into())
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn default_output_is_bluetooth() -> Result<bool, String> {
        Err("speaker suppression is macOS-only".into())
    }

    pub fn current_output_muted() -> Result<bool, String> {
        Err("speaker suppression is macOS-only".into())
    }

    pub fn set_output_muted(_muted: bool) -> Result<(), String> {
        Err("speaker suppression is macOS-only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::RestoreAction;

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }

    fn restore_action(previous_muted: bool, mute_succeeded: bool) -> RestoreAction {
        if previous_muted || !mute_succeeded {
            RestoreAction::Noop
        } else {
            RestoreAction::RestoreUnmuted
        }
    }

    #[test]
    fn speaker_suppression_restores_when_airnote_muted_output() {
        assert_eq!(restore_action(false, true), RestoreAction::RestoreUnmuted);
    }

    #[test]
    fn speaker_suppression_does_not_unmute_user_muted_output() {
        assert_eq!(restore_action(true, true), RestoreAction::Noop);
    }

    #[test]
    fn speaker_suppression_noops_when_mute_fails() {
        assert_eq!(restore_action(false, false), RestoreAction::Noop);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bluetooth_transports_skip_output_mute() {
        assert!(super::platform::is_bluetooth_transport(fourcc(*b"blue")));
        assert!(super::platform::is_bluetooth_transport(fourcc(*b"blea")));
        assert!(!super::platform::is_bluetooth_transport(fourcc(*b"buil")));
        assert!(!super::platform::is_bluetooth_transport(fourcc(*b"usb ")));
    }
}
