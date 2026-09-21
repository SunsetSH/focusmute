//! Observed Solo mapping from docs/20-ledtest.md, not a predicted 2i2 layout.
use crate::device::{DeviceError, Result, ScarlettDevice};

/// Confirmed physical number LEDs from `20-ledtest.md`.
pub const INPUT_1_LED: u8 = 4;
pub const INPUT_2_LED: u8 = 12;
pub const WHITE: u32 = 0xFFFF_FF00;

/// Return the confirmed number LEDs for the configured zero-based input set.
/// Unknown inputs are ignored so a malformed config cannot light a panel LED.
pub fn number_leds(inputs: &[usize]) -> Vec<u8> {
    inputs
        .iter()
        .filter_map(|input| match input {
            0 => Some(INPUT_1_LED),
            1 => Some(INPUT_2_LED),
            _ => None,
        })
        .collect()
}

pub fn label(index: u8) -> &'static str {
    match index {
        0 => "Air",
        2 => "Inst",
        3 => "48V",
        4 => "1",
        6..=11 => "Halo 1",
        12 => "2",
        13 => "Halo ?",
        14..=19 => "Halo 2",
        24..=25 => "Output",
        26 => "USB",
        27 | 31 => "Direct",
        _ => "?",
    }
}

/// Static panel lights have no verified readable RGB state. Test only the
/// number with known white restoration and the observed transient halo LEDs.
pub fn can_test(index: u8) -> bool {
    matches!(index, INPUT_1_LED | INPUT_2_LED)
        || (6..=11).contains(&index)
        || (13..=19).contains(&index)
}

fn require_solo(device: &impl ScarlettDevice) -> Result<()> {
    if !crate::direct::is_solo(device) {
        return Err(DeviceError::UnsupportedDevice(device.info().model().into()));
    }
    Ok(())
}

pub fn test_led(device: &impl ScarlettDevice, index: u8, color: u32) -> Result<()> {
    require_solo(device)?;
    if !can_test(index) {
        return Err(DeviceError::TransactFailed(
            "LED restoration is unverified for this index; test disabled".into(),
        ));
    }
    super::set_single_led(device, index, color)
}

/// No DATA_NOTIFY(5): that command applies the bulk array, including its
/// zero entries. It does not restore the pre-test physical panel colours.
pub fn restore_test(device: &impl ScarlettDevice, inputs: &[usize]) -> Result<()> {
    require_solo(device)?;
    number_leds(inputs)
        .into_iter()
        .try_for_each(|index| super::set_single_led(device, index, WHITE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDevice;

    #[test]
    fn lab_rejects_unrestorable_lights_and_non_solo_without_writes() {
        let mut dev = MockDevice::new();
        assert!(test_led(&dev, 4, WHITE).is_err());
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        for index in [0, 1, 2, 3, 5, 20, 24, 25, 26, 27, 31, 255] {
            assert!(test_led(&dev, index, WHITE).is_err());
        }
        assert!(dev.descriptors.borrow().is_empty());
        assert!(dev.notifies.borrow().is_empty());
    }

    #[test]
    fn restore_never_applies_a_bulk_array_or_changes_audio_modes() {
        let mut dev = MockDevice::new();
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        restore_test(&dev, &[0, 1]).unwrap();
        let writes = dev.descriptors.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes.get(&84), Some(&vec![12]));
        assert_eq!(writes.get(&80), Some(&WHITE.to_le_bytes().to_vec()));
        assert_eq!(*dev.notifies.borrow(), vec![8, 8]);
    }
}
