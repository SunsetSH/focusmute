//! LED device operations — single-LED mute indicator apply/clear/restore.

use crate::device::{Result, ScarlettDevice};
use crate::protocol;

use super::strategy::{IndicatorRenderMode, MuteStrategy};

/// Descriptor fields used by the firmware's single-LED command. The 2i2
/// values remain the legacy defaults; Solo has a compact APP_SPACE layout.
fn single_led_fields(device: &impl ScarlettDevice) -> (u32, u32, u32) {
    if device
        .info()
        .model()
        .eq_ignore_ascii_case("Scarlett Solo 4th Gen")
    {
        // Verified from the Solo 4th Gen firmware schema (fw 2.0.2417.0).
        // colour=80, index=84, notify=8. Do not use 2i2's 84/88 here.
        (80, 84, 8)
    } else {
        (
            protocol::OFF_DIRECT_LED_COLOUR,
            protocol::OFF_DIRECT_LED_INDEX,
            protocol::NOTIFY_DIRECT_LED_COLOUR,
        )
    }
}

fn is_solo(device: &impl ScarlettDevice) -> bool {
    device
        .info()
        .model()
        .eq_ignore_ascii_case("Scarlett Solo 4th Gen")
}

// ── Single-LED update (DATA_NOTIFY(8)) ──

/// Set a single LED color via `directLEDColour` + `directLEDIndex` + DATA_NOTIFY(8).
///
/// Updates ONLY the targeted LED — zero side effects on any other LED.
/// Works in mode 0 (normal metering mode) without any mode change.
/// Metering continues unaffected on all halo ring segments.
pub fn set_single_led(device: &impl ScarlettDevice, index: u8, color: u32) -> Result<()> {
    let (colour_offset, index_offset, notify) = single_led_fields(device);
    // Ordering matters: colour must be written before index.
    device.set_descriptor(colour_offset, &color.to_le_bytes())?;
    device.set_descriptor(index_offset, &[index])?;
    device.data_notify(notify)?;
    Ok(())
}

/// Restore number LEDs to their firmware-expected colors.
///
/// Reads `selectedInput` from the device to determine which input is currently
/// selected, then sets each number LED to the appropriate firmware color
/// (green for selected, off for unselected) via DATA_NOTIFY(8).
fn restore_number_leds(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    // Solo has no selectedInput field. Restore only the number LEDs that
    // FocusMute has overridden; Air, Direct, and other LEDs stay firmware-owned.
    if is_solo(device) {
        return super::solo::number_leds(&strategy.input_indices)
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, super::solo::WHITE));
    }
    let selected_input = device
        .get_descriptor(protocol::OFF_SELECTED_INPUT, 1)?
        .first()
        .copied()
        .unwrap_or(0) as usize;

    for (input_idx, &led_idx) in strategy
        .input_indices
        .iter()
        .zip(strategy.number_leds.iter())
    {
        let color = if *input_idx == selected_input {
            strategy.selected_color
        } else {
            strategy.unselected_color
        };
        set_single_led(device, led_idx, color)?;
    }
    Ok(())
}

/// The Solo direct-LED bulk-array map has not yet been calibrated from the
/// attached hardware. Keep the normal firmware renderer in control until the
/// laboratory observations provide a verified map.
pub fn blank_mute_indicator(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    if is_solo(device) {
        return super::solo::number_leds(&strategy.input_indices)
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, 0));
    }
    strategy
        .number_leds
        .iter()
        .try_for_each(|&index| set_single_led(device, index, 0))
}

// ── Mute indicator operations ──

/// Apply the mute indicator based on the resolved strategy.
///
/// Sets only the muted input number LEDs via DATA_NOTIFY(8).
/// No mode change, no gradient change — metering continues on all other LEDs.
pub fn apply_mute_indicator(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    mute_color: u32,
) -> Result<()> {
    if is_solo(device) {
        // The observed map supersedes the generic predicted layout (0/8).
        return strategy
            .input_indices
            .iter()
            .enumerate()
            .filter_map(|(position, input)| match input {
                0 => Some((super::solo::INPUT_1_LED, position)),
                1 => Some((super::solo::INPUT_2_LED, position)),
                _ => None,
            })
            .try_for_each(|(led, position)| {
                set_single_led(
                    device,
                    led,
                    strategy
                        .mute_colors
                        .get(position)
                        .copied()
                        .unwrap_or(mute_color),
                )
            });
    }
    for (i, &led_idx) in strategy.number_leds.iter().enumerate() {
        let color = strategy.mute_colors.get(i).copied().unwrap_or(mute_color);
        set_single_led(device, led_idx, color)?;
    }
    Ok(())
}

/// Apply the selected rendering. Kept separate from the legacy API so all
/// existing 2i2 callers and integrations retain number-indicator behaviour.
pub fn apply_mute_indicator_with_mode(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    mute_color: u32,
    mode: IndicatorRenderMode,
) -> Result<()> {
    match mode {
        IndicatorRenderMode::Extended => apply_extended_indicator(device, strategy, mute_color),
        IndicatorRenderMode::HalosSolid => {
            if is_solo(device) {
                log::warn!(
                    "[led] Solo firmware overwrites halo colours; using the verified number indicator"
                );
            } else {
                log::warn!("[led] solid halos are unsupported; using number LEDs");
            }
            apply_mute_indicator(device, strategy, mute_color)
        }
        IndicatorRenderMode::Numbers
        | IndicatorRenderMode::NumbersBlink
        | IndicatorRenderMode::Auto => apply_mute_indicator(device, strategy, mute_color),
    }
}

/// Extended indication owns both input number LEDs and both Output indicators.
fn apply_extended_indicator(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    color: u32,
) -> Result<()> {
    if is_solo(device) {
        return [super::solo::INPUT_1_LED, super::solo::INPUT_2_LED, 24, 25]
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, color));
    }
    if device
        .info()
        .model()
        .eq_ignore_ascii_case("Scarlett 2i2 4th Gen")
    {
        return [0, 8, 37, 38]
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, color));
    }
    strategy
        .number_leds
        .iter()
        .try_for_each(|&index| set_single_led(device, index, color))
}

fn clear_extended_indicator(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    if is_solo(device) {
        return [super::solo::INPUT_1_LED, super::solo::INPUT_2_LED, 24, 25]
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, super::solo::WHITE));
    }
    if device
        .info()
        .model()
        .eq_ignore_ascii_case("Scarlett 2i2 4th Gen")
    {
        let selected = device
            .get_descriptor(protocol::OFF_SELECTED_INPUT, 1)?
            .first()
            .copied()
            .unwrap_or(0) as usize;
        for (input, led) in [(0, 0), (1, 8)] {
            set_single_led(
                device,
                led,
                if input == selected {
                    strategy.selected_color
                } else {
                    strategy.unselected_color
                },
            )?;
        }
        return [37, 38]
            .into_iter()
            .try_for_each(|index| set_single_led(device, index, 0x7080_8800));
    }
    clear_mute_indicator(device, strategy)
}
/// Clear the mute indicator and restore normal LED state.
pub fn clear_mute_indicator(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    restore_number_leds(device, strategy)
}

pub fn clear_mute_indicator_with_mode(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    mode: IndicatorRenderMode,
) -> Result<()> {
    if mode == IndicatorRenderMode::Extended {
        return clear_extended_indicator(device, strategy);
    }
    clear_mute_indicator(device, strategy)
}

/// Restore LED state on application exit.
pub fn restore_on_exit(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    restore_number_leds(device, strategy)
}

/// Re-apply mute indicator after reconnecting, if currently muted.
///
/// The caller is responsible for the `open_device()` call and logging —
/// this extracts only the post-connect mute re-application.
pub fn refresh_after_reconnect(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    mute_color: u32,
    is_muted: bool,
) -> Result<()> {
    if is_muted {
        apply_mute_indicator(device, strategy, mute_color)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDevice;
    use crate::protocol::*;

    /// Helper to set up a mock device with selectedInput for restore tests.
    fn setup_device_with_selected_input(dev: &MockDevice, selected: u8) {
        dev.set_descriptor(OFF_SELECTED_INPUT, &[selected]).unwrap();
    }

    fn make_strategy_one_input() -> MuteStrategy {
        MuteStrategy {
            input_indices: vec![0],
            number_leds: vec![0],
            mute_colors: vec![],
            selected_color: 0x20FF_0000,
            unselected_color: 0x88FF_FF00,
        }
    }

    fn make_strategy_both_inputs() -> MuteStrategy {
        MuteStrategy {
            input_indices: vec![0, 1],
            number_leds: vec![0, 8],
            mute_colors: vec![],
            selected_color: 0x20FF_0000,
            unselected_color: 0x88FF_FF00,
        }
    }

    fn solo_device() -> MockDevice {
        let mut dev = MockDevice::new();
        dev.info_mut().device_name = "Scarlett Solo 4th Gen-TEST".into();
        dev
    }

    // ── set_single_led ──

    #[test]
    fn set_single_led_writes_colour_index_notify() {
        let dev = MockDevice::new();
        let color = 0xFF00_0000u32;
        set_single_led(&dev, 0, color).unwrap();

        let descs = dev.descriptors.borrow();

        // directLEDColour should be written
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(u32::from_le_bytes(colour[..4].try_into().unwrap()), color);

        // directLEDIndex should be written
        let index = descs.get(&OFF_DIRECT_LED_INDEX).unwrap();
        assert_eq!(index, &[0]);

        // Should have sent NOTIFY_DIRECT_LED_COLOUR (8)
        let notifies = dev.notifies.borrow();
        assert!(notifies.contains(&NOTIFY_DIRECT_LED_COLOUR));
    }

    #[test]
    fn set_single_led_does_not_touch_mode_or_values() {
        let dev = MockDevice::new();
        set_single_led(&dev, 0, 0xFF00_0000).unwrap();

        let descs = dev.descriptors.borrow();
        assert!(!descs.contains_key(&OFF_ENABLE_DIRECT_LED));
        assert!(!descs.contains_key(&OFF_DIRECT_LED_VALUES));
    }

    #[test]
    fn solo_single_led_uses_solo_fields_without_2i2_value_field() {
        let dev = solo_device();
        set_single_led(&dev, 7, 0x12_34_56_00).unwrap();

        let descriptors = dev.descriptors.borrow();
        assert_eq!(
            descriptors.get(&80),
            Some(&0x12_34_56_00u32.to_le_bytes().to_vec())
        );
        assert_eq!(descriptors.get(&84), Some(&vec![7]));
        assert!(!descriptors.contains_key(&88));
        assert_eq!(dev.notifies.borrow().as_slice(), &[8]);
    }

    #[test]
    fn solo_auto_mode_keeps_firmware_halos_and_restores_number_to_white() {
        let dev = solo_device();
        let color = 0xAA_BB_CC_00;
        apply_mute_indicator_with_mode(
            &dev,
            &make_strategy_one_input(),
            color,
            IndicatorRenderMode::Auto,
        )
        .unwrap();

        let descriptors = dev.descriptors.borrow();
        assert!(!descriptors.contains_key(&72));
        assert!(!descriptors.contains_key(&88));
        assert_eq!(descriptors.get(&80), Some(&color.to_le_bytes().to_vec()));
        assert_eq!(descriptors.get(&84), Some(&vec![4]));
        drop(descriptors);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8]);

        clear_mute_indicator_with_mode(&dev, &make_strategy_one_input(), IndicatorRenderMode::Auto)
            .unwrap();
        let descriptors = dev.descriptors.borrow();
        assert_eq!(
            descriptors.get(&80),
            Some(&0xFFFF_FF00u32.to_le_bytes().to_vec())
        );
        assert_eq!(descriptors.get(&84), Some(&vec![4]));
        drop(descriptors);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8, 8]);
    }

    #[test]
    fn solo_second_input_and_legacy_halo_config_never_target_air_or_halos() {
        let dev = solo_device();
        let strategy = MuteStrategy {
            input_indices: vec![1],
            number_leds: vec![8], // A generic prediction must never reach Solo.
            mute_colors: vec![0x12_34_56_00],
            selected_color: 0,
            unselected_color: 0,
        };
        for mode in [
            IndicatorRenderMode::Auto,
            IndicatorRenderMode::Numbers,
            IndicatorRenderMode::NumbersBlink,
            IndicatorRenderMode::HalosSolid,
        ] {
            apply_mute_indicator_with_mode(&dev, &strategy, 0xFF00_0000, mode).unwrap();
            assert_eq!(
                dev.descriptors.borrow().get(&84),
                Some(&vec![super::super::solo::INPUT_2_LED])
            );
            blank_mute_indicator(&dev, &strategy).unwrap();
            assert_eq!(
                dev.descriptors.borrow().get(&84),
                Some(&vec![super::super::solo::INPUT_2_LED])
            );
            assert_eq!(dev.descriptors.borrow().get(&80), Some(&vec![0; 4]));
            clear_mute_indicator_with_mode(&dev, &strategy, mode).unwrap();
            assert_eq!(
                dev.descriptors.borrow().get(&84),
                Some(&vec![super::super::solo::INPUT_2_LED])
            );
        }
        assert_eq!(dev.descriptors.borrow().len(), 2);
        assert!(dev.notifies.borrow().iter().all(|&event| event == 8));
        assert_eq!(strategy.input_indices, vec![1]);
    }

    // ── apply_mute_indicator ──

    #[test]
    fn apply_mute_sets_only_number_led() {
        let dev = MockDevice::new();
        let strategy = make_strategy_one_input();
        let color = 0xFF00_0000u32;

        apply_mute_indicator(&dev, &strategy, color).unwrap();

        let descs = dev.descriptors.borrow();

        // directLEDColour should have mute color
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(u32::from_le_bytes(colour[..4].try_into().unwrap()), color);

        // directLEDIndex should be 0 (input 1 number LED)
        let index = descs.get(&OFF_DIRECT_LED_INDEX).unwrap();
        assert_eq!(index, &[0]);

        // Mode should NOT be changed
        assert!(!descs.contains_key(&OFF_ENABLE_DIRECT_LED));

        // directLEDValues should NOT be changed
        assert!(!descs.contains_key(&OFF_DIRECT_LED_VALUES));

        // Only NOTIFY_DIRECT_LED_COLOUR (8) should have been sent
        let notifies = dev.notifies.borrow();
        assert!(notifies.contains(&NOTIFY_DIRECT_LED_COLOUR));
        assert!(!notifies.contains(&NOTIFY_DIRECT_LED_VALUES));
    }

    #[test]
    fn apply_mute_both_number_leds() {
        let dev = MockDevice::new();
        let strategy = make_strategy_both_inputs();
        let color = 0xFF00_0000u32;

        apply_mute_indicator(&dev, &strategy, color).unwrap();

        // Both LEDs should have been set
        let notifies = dev.notifies.borrow();
        assert_eq!(
            notifies
                .iter()
                .filter(|&&n| n == NOTIFY_DIRECT_LED_COLOUR)
                .count(),
            2,
            "should have sent 2 DATA_NOTIFY(8) events"
        );
    }

    #[test]
    fn solo_all_inputs_targets_only_both_confirmed_channel_numbers() {
        let dev = solo_device();
        apply_mute_indicator(&dev, &make_strategy_both_inputs(), 0xFF00_0000).unwrap();

        // MockDevice retains the final single-LED index; two notifications
        // prove both known Solo channel numbers were issued, with no halo write.
        assert_eq!(
            dev.descriptors.borrow().get(&84),
            Some(&vec![super::super::solo::INPUT_2_LED])
        );
        assert_eq!(dev.notifies.borrow().as_slice(), &[8, 8]);
    }

    #[test]
    fn apply_mute_uses_per_input_colors() {
        let dev = MockDevice::new();
        let strategy = MuteStrategy {
            input_indices: vec![0, 1],
            number_leds: vec![0, 8],
            mute_colors: vec![0x00FF_0000, 0x0000_FF00],
            selected_color: 0x20FF_0000,
            unselected_color: 0x88FF_FF00,
        };

        apply_mute_indicator(&dev, &strategy, 0xFF00_0000).unwrap();

        // Last written colour should be for input 2 (0x0000_FF00)
        let descs = dev.descriptors.borrow();
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(
            u32::from_le_bytes(colour[..4].try_into().unwrap()),
            0x0000_FF00
        );
    }

    // ── clear_mute_indicator ──

    #[test]
    fn clear_mute_restores_number_led_selected() {
        let dev = MockDevice::new();
        let strategy = make_strategy_one_input();

        // Input 1 is selected
        setup_device_with_selected_input(&dev, 0);

        // Apply then clear
        apply_mute_indicator(&dev, &strategy, 0xFF00_0000).unwrap();
        clear_mute_indicator(&dev, &strategy).unwrap();

        let descs = dev.descriptors.borrow();

        // Number LED should be restored to selected color (green)
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(
            u32::from_le_bytes(colour[..4].try_into().unwrap()),
            0x20FF_0000,
            "should restore to selected green"
        );

        // Mode should NOT have been touched
        assert!(!descs.contains_key(&OFF_ENABLE_DIRECT_LED));
    }

    #[test]
    fn clear_mute_restores_number_led_unselected() {
        let dev = MockDevice::new();
        let strategy = make_strategy_one_input();

        // Input 2 is selected (so input 1 is unselected)
        setup_device_with_selected_input(&dev, 1);

        apply_mute_indicator(&dev, &strategy, 0xFF00_0000).unwrap();
        clear_mute_indicator(&dev, &strategy).unwrap();

        let descs = dev.descriptors.borrow();

        // Number LED should be restored to unselected color (white)
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(
            u32::from_le_bytes(colour[..4].try_into().unwrap()),
            0x88FF_FF00,
            "should restore to unselected (white)"
        );
    }

    #[test]
    fn clear_mute_both_inputs_correct_colors() {
        let dev = MockDevice::new();
        let strategy = make_strategy_both_inputs();

        // Input 1 is selected
        setup_device_with_selected_input(&dev, 0);

        apply_mute_indicator(&dev, &strategy, 0xFF00_0000).unwrap();
        clear_mute_indicator(&dev, &strategy).unwrap();

        // Should have sent 4 DATA_NOTIFY(8) events total (2 apply + 2 clear)
        let notifies = dev.notifies.borrow();
        assert_eq!(
            notifies
                .iter()
                .filter(|&&n| n == NOTIFY_DIRECT_LED_COLOUR)
                .count(),
            4,
        );

        // Last LED written was index 8 (input 2, unselected → white)
        let descs = dev.descriptors.borrow();
        let index = descs.get(&OFF_DIRECT_LED_INDEX).unwrap();
        assert_eq!(index, &[8]);
        let colour = descs.get(&OFF_DIRECT_LED_COLOUR).unwrap();
        assert_eq!(
            u32::from_le_bytes(colour[..4].try_into().unwrap()),
            0x88FF_FF00,
            "last LED restored should be unselected (white)"
        );
    }

    // ── restore_on_exit ──

    #[test]
    fn restore_on_exit_restores_via_single_led() {
        let dev = MockDevice::new();
        let strategy = make_strategy_one_input();

        // Input 1 is selected
        setup_device_with_selected_input(&dev, 0);

        restore_on_exit(&dev, &strategy).unwrap();

        let descs = dev.descriptors.borrow();

        // Should restore via DATA_NOTIFY(8), not bulk mode/values
        assert!(descs.contains_key(&OFF_DIRECT_LED_COLOUR));
        assert!(!descs.contains_key(&OFF_ENABLE_DIRECT_LED));
        assert!(!descs.contains_key(&OFF_DIRECT_LED_VALUES));
    }

    // ── refresh_after_reconnect ──

    #[test]
    fn refresh_after_reconnect_not_muted() {
        let dev = MockDevice::new();
        let strategy = make_strategy_both_inputs();

        refresh_after_reconnect(&dev, &strategy, 0xFF00_0000, false).unwrap();

        // Not muted → no directLEDColour written
        let descs = dev.descriptors.borrow();
        assert!(
            !descs.contains_key(&OFF_DIRECT_LED_COLOUR),
            "should not write directLEDColour when not muted"
        );
    }

    #[test]
    fn refresh_after_reconnect_muted() {
        let dev = MockDevice::new();
        let strategy = make_strategy_both_inputs();
        let mute_color = 0xFF00_0000u32;

        refresh_after_reconnect(&dev, &strategy, mute_color, true).unwrap();

        let descs = dev.descriptors.borrow();
        // Muted → directLEDColour should have been written with mute color
        let colour = descs
            .get(&OFF_DIRECT_LED_COLOUR)
            .expect("should write directLEDColour when muted");
        assert_eq!(
            u32::from_le_bytes(colour[..4].try_into().unwrap()),
            mute_color
        );

        // Should have sent NOTIFY_DIRECT_LED_COLOUR for both LEDs
        let notifies = dev.notifies.borrow();
        assert_eq!(
            notifies
                .iter()
                .filter(|&&n| n == NOTIFY_DIRECT_LED_COLOUR)
                .count(),
            2,
            "should send DATA_NOTIFY(8) for each number LED"
        );
    }

    #[test]
    fn refresh_after_reconnect_muted_apply_fails_returns_err() {
        let dev = MockDevice::new();
        let strategy = make_strategy_both_inputs();

        // Make set_descriptor fail
        dev.fail_set_descriptor.set(true);

        // Should propagate the error from apply_mute_indicator
        let result = refresh_after_reconnect(&dev, &strategy, 0xFF00_0000, true);
        assert!(result.is_err(), "should return Err when apply fails");
    }

    // ── T3: set_single_led error propagation ──

    #[test]
    fn set_single_led_propagates_write_error() {
        let dev = MockDevice::new();
        dev.fail_set_descriptor.set(true);

        let result = set_single_led(&dev, 0, 0xFF00_0000);
        assert!(result.is_err(), "should propagate set_descriptor error");
    }

    #[test]
    fn set_single_led_propagates_notify_error() {
        let dev = MockDevice::new();
        dev.fail_data_notify.set(true);

        let result = set_single_led(&dev, 0, 0xFF00_0000);
        assert!(result.is_err(), "should propagate data_notify error");
    }
}
