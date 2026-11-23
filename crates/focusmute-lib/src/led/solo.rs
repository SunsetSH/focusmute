//! Solo LED Lab: single-LED tests on the observed Solo panel map
//! (docs/20-ledtest.md, docs/21), not a predicted 2i2 layout.
//!
//! Every entry point takes the device's [`Panel`], which only a Solo on a
//! verified firmware layout has, so the Lab cannot write to any other device.
use crate::device::{DeviceError, Result, ScarlettDevice};
use crate::models::Panel;

/// Confirmed physical number LEDs from `20-ledtest.md`.
pub const INPUT_1_LED: u8 = 4;
pub const INPUT_2_LED: u8 = 12;
/// Number-indicator white tuned on Scarlett 2i2 and used for Solo restoration.
pub const WHITE: u32 = 0xAAFF_DD00;

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
        5..=11 => "Halo 1",
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

fn require_panel(device: &impl ScarlettDevice, panel: Option<Panel>) -> Result<Panel> {
    panel.ok_or_else(|| DeviceError::UnsupportedDevice(device.info().model().into()))
}

pub fn test_led(
    device: &impl ScarlettDevice,
    panel: Option<Panel>,
    index: u8,
    color: u32,
) -> Result<()> {
    let panel = require_panel(device, panel)?;
    if !can_test(index) {
        return Err(DeviceError::TransactFailed(
            "LED restoration is unverified for this index; test disabled".into(),
        ));
    }
    super::set_single_led(device, panel.offsets, index, color)
}

/// No DATA_NOTIFY(5): that command applies the bulk array, including its
/// zero entries. It does not restore the pre-test physical panel colours.
pub fn restore_test(
    device: &impl ScarlettDevice,
    panel: Option<Panel>,
    inputs: &[usize],
) -> Result<()> {
    let panel = require_panel(device, panel)?;
    number_leds(inputs)
        .into_iter()
        .try_for_each(|index| super::set_single_led(device, panel.offsets, index, WHITE))
}

/// Read-only snapshot of the LED mode, Air, Direct and the bulk LED command
/// buffer: `(mode, air, direct, values)`.
pub fn snapshot(
    device: &impl ScarlettDevice,
    panel: Option<Panel>,
) -> Result<(u8, u8, u8, Vec<u8>)> {
    let panel = require_panel(device, panel)?;
    let byte = |offset| -> Result<u8> {
        Ok(device
            .get_descriptor(offset, 1)?
            .first()
            .copied()
            .unwrap_or(0))
    };
    let mode = byte(panel.offsets.enable_direct_led)?;
    let direct = byte(panel.controls.direct_monitoring)?;
    let values = device.get_descriptor(
        panel.offsets.direct_led_values,
        panel.offsets.direct_led_size(),
    )?;
    let air = byte(panel.controls.input_air)?;
    Ok((mode, air, direct, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDevice;

    fn solo() -> (MockDevice, Option<Panel>) {
        let mut dev = MockDevice::new();
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        let panel = crate::models::panel(dev.info());
        (dev, panel)
    }

    #[test]
    fn lab_rejects_unrestorable_lights_and_non_solo_without_writes() {
        let dev = MockDevice::new();
        let panel = crate::models::panel(dev.info());
        assert!(test_led(&dev, panel, 4, WHITE).is_err());
        let (dev, panel) = solo();
        for index in [0, 1, 2, 3, 5, 20, 24, 25, 26, 27, 31, 255] {
            assert!(test_led(&dev, panel, index, WHITE).is_err());
        }
        assert!(dev.descriptors.borrow().is_empty());
        assert!(dev.notifies.borrow().is_empty());
    }

    #[test]
    fn restore_never_applies_a_bulk_array_or_changes_audio_modes() {
        let (dev, panel) = solo();
        restore_test(&dev, panel, &[0, 1]).unwrap();
        let writes = dev.descriptors.borrow();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes.get(&84), Some(&vec![12]));
        assert_eq!(writes.get(&80), Some(&WHITE.to_le_bytes().to_vec()));
        assert_eq!(*dev.notifies.borrow(), vec![8, 8]);
    }

    #[test]
    fn snapshot_reads_the_solo_fields_and_writes_nothing() {
        use crate::device::mock::MockOp;
        let (dev, panel) = solo();
        snapshot(&dev, panel).unwrap();
        assert_eq!(
            *dev.ops.borrow(),
            vec![
                MockOp::Get(72, 1),
                MockOp::Get(264, 1),
                MockOp::Get(88, 128),
                MockOp::Get(62, 1),
            ]
        );
    }
}
