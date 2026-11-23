//! Direct Monitor control and notification decoding.
//!
//! Only models whose profile declares [`PanelControls`] get here; callers
//! obtain them from [`crate::models::panel`].

use crate::device::{Result, ScarlettDevice};
use crate::models::PanelControls;

/// Decode the SwRoot notification packet. Firmware event flags occupy bytes
/// 4..8 in little-endian order.
pub fn notification_mask(packet: &[u8]) -> Option<u32> {
    packet
        .get(4..8)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
}

/// Read the current Direct Monitor state.
pub fn direct_enabled(device: &impl ScarlettDevice, panel: &PanelControls) -> Result<bool> {
    Ok(device
        .get_descriptor(panel.direct_monitoring, 1)?
        .first()
        .copied()
        .unwrap_or(0)
        != 0)
}

/// Set Direct Monitor through the parameter buffer. The `directMonitoring`
/// scalar is readable, but the firmware ignores direct writes to it.
pub fn set_direct_enabled(
    device: &impl ScarlettDevice,
    panel: &PanelControls,
    enabled: bool,
) -> Result<()> {
    device.set_descriptor(panel.parameter_channel, &[0])?;
    device.set_descriptor(panel.parameter_value, &[u8::from(enabled)])?;
    device.data_notify(panel.direct_monitoring_notify)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDevice;

    fn solo_panel() -> &'static PanelControls {
        crate::models::detect_model("Scarlett Solo 4th Gen")
            .and_then(|p| p.panel)
            .unwrap()
    }

    #[test]
    fn notification_mask_uses_little_endian_event_bytes() {
        let packet = [0, 0, 0, 0, 0, 0, 0x80, 0];
        assert_eq!(
            notification_mask(&packet),
            Some(solo_panel().direct_notify_mask)
        );
        assert_eq!(notification_mask(&packet[..7]), None);
    }

    #[test]
    fn set_direct_uses_solo_parameter_buffer() {
        let dev = MockDevice::new();
        set_direct_enabled(&dev, solo_panel(), true).unwrap();

        let descriptors = dev.descriptors.borrow();
        assert_eq!(descriptors.get(&217), Some(&vec![0]));
        assert_eq!(descriptors.get(&216), Some(&vec![1]));
        assert_eq!(dev.notifies.borrow().as_slice(), &[12]);
    }

    #[test]
    fn direct_state_is_read_from_direct_monitoring() {
        let dev = MockDevice::new();
        dev.set_descriptor(264, &[1]).unwrap();
        assert!(direct_enabled(&dev, solo_panel()).unwrap());
    }

    /// The 2i2's Direct button has other modes and notify bits, so it must
    /// never reach the parameter-buffer write above.
    #[test]
    fn only_the_solo_declares_panel_controls() {
        let mut dev = MockDevice::new();
        assert!(crate::models::panel(dev.info()).is_none());
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        assert!(crate::models::panel(dev.info()).is_some());
    }
}
