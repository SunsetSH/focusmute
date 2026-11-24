//! `mute` / `unmute` subcommands — set OS microphone mute state.

use super::{Result, audio};

pub(super) enum MuteAction {
    Mute,
    Unmute,
}

/// Set OS mute state. LED feedback and sound are handled by the tray's
/// polling loop when it is running; the CLI intentionally does not duplicate
/// that to avoid double-firing.
///
/// `prefer_scarlett` picks the microphone as the tray does
/// (`[system].scarlett_microphone` of the config in use).
pub(super) fn cmd_set_mute(action: MuteAction, prefer_scarlett: bool) -> Result<()> {
    let target = match action {
        MuteAction::Mute => true,
        MuteAction::Unmute => false,
    };

    #[cfg(windows)]
    {
        use super::MuteMonitor;
        audio::com_init()?;
        audio::WasapiMonitor::new(prefer_scarlett)?.set_muted(target)?;
    }

    #[cfg(target_os = "linux")]
    {
        use super::MuteMonitor;
        let _ = prefer_scarlett;
        let monitor = audio::PulseAudioMonitor::new()?;
        audio::stabilize_pulseaudio(&monitor);
        monitor.set_muted(target)?;
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (target, prefer_scarlett);
        return Err(focusmute_lib::FocusmuteError::Audio(
            focusmute_lib::audio::AudioError::InitFailed(
                "Mute control is not yet supported on this platform.".into(),
            ),
        ));
    }

    println!("Microphone: {}", if target { "MUTED" } else { "UNMUTED" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_action_to_bool() {
        let mute_target = match MuteAction::Mute {
            MuteAction::Mute => true,
            MuteAction::Unmute => false,
        };
        assert!(mute_target);

        let unmute_target = match MuteAction::Unmute {
            MuteAction::Mute => true,
            MuteAction::Unmute => false,
        };
        assert!(!unmute_target);
    }
}
