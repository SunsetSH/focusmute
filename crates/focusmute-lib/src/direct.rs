//! Scarlett Solo Direct Monitor control and notification decoding.

use crate::device::{Result, ScarlettDevice};

pub const SOLO_DIRECT_NOTIFY_MASK: u32 = 0x0080_0000;
pub const SOLO_VIDEO_CALL_NOTIFY_MASK: u32 = 0x0400_0000;

pub fn is_solo(device: &impl ScarlettDevice) -> bool {
    device
        .info()
        .model()
        .eq_ignore_ascii_case("Scarlett Solo 4th Gen")
}

/// Decode the SwRoot notification packet. Firmware event flags occupy bytes
/// 4..8 in little-endian order.
pub fn notification_mask(packet: &[u8]) -> Option<u32> {
    packet
        .get(4..8)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
}

pub fn solo_direct_enabled(device: &impl ScarlettDevice) -> Result<bool> {
    Ok(device.get_descriptor(264, 1)?.first().copied().unwrap_or(0) != 0)
}

/// Set Solo Direct Monitor through its parameter buffer. The scalar at 264 is
/// readable, but direct writes to it are ignored by the Solo firmware.
pub fn set_solo_direct_enabled(device: &impl ScarlettDevice, enabled: bool) -> Result<()> {
    if !is_solo(device) {
        return Ok(());
    }
    device.set_descriptor(217, &[0])?;
    device.set_descriptor(216, &[u8::from(enabled)])?;
    device.data_notify(12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDevice;

    fn solo_device() -> MockDevice {
        let mut dev = MockDevice::new();
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        dev
    }

    #[test]
    fn notification_mask_uses_little_endian_event_bytes() {
        let packet = [0, 0, 0, 0, 0, 0, 0x80, 0];
        assert_eq!(notification_mask(&packet), Some(SOLO_DIRECT_NOTIFY_MASK));
        assert_eq!(notification_mask(&packet[..7]), None);
    }

    #[test]
    fn set_direct_uses_solo_parameter_buffer() {
        let dev = solo_device();
        set_solo_direct_enabled(&dev, true).unwrap();

        let descriptors = dev.descriptors.borrow();
        assert_eq!(descriptors.get(&217), Some(&vec![0]));
        assert_eq!(descriptors.get(&216), Some(&vec![1]));
        assert_eq!(dev.notifies.borrow().as_slice(), &[12]);
    }

    #[test]
    fn direct_write_is_ignored_for_non_solo() {
        let dev = MockDevice::new();
        set_solo_direct_enabled(&dev, true).unwrap();
        assert!(dev.descriptors.borrow().is_empty());
        assert!(dev.notifies.borrow().is_empty());
    }
}
