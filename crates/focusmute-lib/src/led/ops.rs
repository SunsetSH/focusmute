//! LED device operations — single-LED mute indicator apply/clear/restore.
//!
//! Nothing here knows model names. Which LEDs to light comes from the
//! [`MuteStrategy`] (built from the model profile or a predicted layout), and
//! where to write them comes from its [`DeviceOffsets`].

use crate::device::{Result, ScarlettDevice};
use crate::offsets::DeviceOffsets;

use super::strategy::{IndicatorRenderMode, MuteStrategy};

// ── Single-LED update (DATA_NOTIFY(8)) ──

/// Set a single LED color via `directLEDColour` + `directLEDIndex` + DATA_NOTIFY.
///
/// Updates ONLY the targeted LED — zero side effects on any other LED.
/// Works in mode 0 (normal metering mode) without any mode change.
/// Metering continues unaffected on all halo ring segments.
///
/// `offsets` must be the connected device's own, from [`DeviceOffsets`]. The
/// three fields differ between models (the 2i2 writes colour at 84 and index at
/// 88; the Solo writes them at 80 and 84), and using another model's values
/// writes the colour into whatever field sits at that offset instead.
pub fn set_single_led(
    device: &impl ScarlettDevice,
    offsets: &DeviceOffsets,
    index: u8,
    color: u32,
) -> Result<()> {
    // Ordering matters: colour must be written before index.
    device.set_descriptor(offsets.direct_led_colour, &color.to_le_bytes())?;
    device.set_descriptor(offsets.direct_led_index, &[index])?;
    device.data_notify(offsets.direct_led_colour_notify)?;
    Ok(())
}

/// The input the firmware currently highlights, or `None` on a model without
/// a `selectedInput` control. Reading the 2i2's offset on such a device would
/// return an unrelated field and pick a colour from it.
fn selected_input(device: &impl ScarlettDevice, offsets: &DeviceOffsets) -> Result<Option<usize>> {
    let Some(offset) = offsets.selected_input else {
        return Ok(None);
    };
    Ok(Some(
        device
            .get_descriptor(offset, 1)?
            .first()
            .copied()
            .unwrap_or(0) as usize,
    ))
}

/// Write each number LED back to the colour the firmware shows for it: the
/// selected input's colour, or the unselected one (white). Models without a
/// `selectedInput` control return every number to white.
fn restore_numbers(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    inputs: &[usize],
    leds: &[u8],
) -> Result<()> {
    let selected = selected_input(device, &strategy.offsets)?;
    for (&input, &led) in inputs.iter().zip(leds) {
        let color = if selected == Some(input) {
            strategy.selected_color
        } else {
            strategy.unselected_color
        };
        set_single_led(device, &strategy.offsets, led, color)?;
    }
    Ok(())
}

/// Restore number LEDs to their firmware-expected colors.
///
/// Only the numbers FocusMute overrode are written; Air, Direct and the other
/// panel LEDs stay firmware-owned.
fn restore_number_leds(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    restore_numbers(
        device,
        strategy,
        &strategy.input_indices,
        &strategy.number_leds,
    )
}

/// Turn the muted number LEDs off — the dark half of a mute blink.
///
/// Uses the single-LED path only: the bulk `directLEDValues` array is not
/// calibrated on every model, and DATA_NOTIFY(5) blanks independent LEDs on
/// the Solo.
pub fn blank_mute_indicator(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    strategy
        .number_leds
        .iter()
        .try_for_each(|&index| set_single_led(device, &strategy.offsets, index, 0))
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
    for (i, &led_idx) in strategy.number_leds.iter().enumerate() {
        let color = strategy.mute_colors.get(i).copied().unwrap_or(mute_color);
        set_single_led(device, &strategy.offsets, led_idx, color)?;
    }
    Ok(())
}

/// Apply the selected rendering. Kept separate from the legacy API so all
/// existing callers and integrations retain number-indicator behaviour.
pub fn apply_mute_indicator_with_mode(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    mute_color: u32,
    mode: IndicatorRenderMode,
) -> Result<()> {
    match mode {
        IndicatorRenderMode::Extended => apply_extended_indicator(device, strategy, mute_color),
        IndicatorRenderMode::HalosSolid => {
            // The firmware repaints halo colours from its own meter state
            // (the Solo overwrites them within a frame), so a solid halo
            // cannot be held. The numbers are the reliable indicator.
            log::warn!("[led] solid halos are unsupported; using number LEDs");
            apply_mute_indicator(device, strategy, mute_color)
        }
        IndicatorRenderMode::Numbers
        | IndicatorRenderMode::NumbersBlink
        | IndicatorRenderMode::Auto => apply_mute_indicator(device, strategy, mute_color),
    }
}

/// Extended indication owns every input number LED and the Output indicator,
/// all in the global mute colour.
fn apply_extended_indicator(
    device: &impl ScarlettDevice,
    strategy: &MuteStrategy,
    color: u32,
) -> Result<()> {
    let Some(ext) = strategy.extended.as_ref() else {
        return strategy
            .number_leds
            .iter()
            .try_for_each(|&index| set_single_led(device, &strategy.offsets, index, color));
    };
    ext.number_leds
        .iter()
        .chain(&ext.output_leds)
        .try_for_each(|&index| set_single_led(device, &strategy.offsets, index, color))
}

fn clear_extended_indicator(device: &impl ScarlettDevice, strategy: &MuteStrategy) -> Result<()> {
    let Some(ext) = strategy.extended.as_ref() else {
        return clear_mute_indicator(device, strategy);
    };
    restore_numbers(device, strategy, &ext.input_indices, &ext.number_leds)?;
    ext.output_leds.iter().try_for_each(|&index| {
        set_single_led(device, &strategy.offsets, index, ext.output_restore_color)
    })
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
            offsets: Default::default(),
            extended: None,
        }
    }

    fn make_strategy_both_inputs() -> MuteStrategy {
        MuteStrategy {
            input_indices: vec![0, 1],
            number_leds: vec![0, 8],
            mute_colors: vec![],
            selected_color: 0x20FF_0000,
            unselected_color: 0x88FF_FF00,
            offsets: Default::default(),
            extended: None,
        }
    }

    /// The strategy the tray resolves for a Solo: its profile's numbers and
    /// offsets, for the given zero-based inputs.
    fn solo_strategy(inputs: &[usize]) -> MuteStrategy {
        let profile = crate::models::detect_model("Scarlett Solo 4th Gen").unwrap();
        let mute_inputs = if inputs.is_empty() {
            crate::config::MuteInputs::All
        } else {
            crate::config::MuteInputs::Specific(inputs.to_vec())
        };
        super::super::strategy::resolve_mute_strategy(
            &mute_inputs,
            Some(profile),
            None,
            0xFF00_0000,
            &Default::default(),
            profile.offsets,
        )
        .unwrap()
        .0
    }

    const SOLO_WHITE: u32 = 0xAAFF_DD00;

    // ── set_single_led ──

    /// Offsets observed on a Scarlett Solo 4th Gen: the single-LED fields sit
    /// four bytes earlier than the 2i2's, and the bulk array starts where the
    /// 2i2 keeps its index field. See docs/13, "Other 4th Gen models".
    fn compact_layout_offsets() -> DeviceOffsets {
        DeviceOffsets {
            direct_led_colour: 80,
            direct_led_index: 84,
            direct_led_colour_notify: 8,
            direct_led_values: 88,
            direct_led_count: 32,
            selected_input: None,
            ..Default::default()
        }
    }

    /// Regression: the write path used the 2i2's constants for every model, so
    /// on a device with a compact APP_SPACE the colour landed in the index
    /// field and the index in the first bulk-array entry.
    #[test]
    fn set_single_led_uses_the_devices_own_offsets() {
        let dev = MockDevice::new();
        let offsets = compact_layout_offsets();
        let color = 0x1234_5600u32;

        set_single_led(&dev, &offsets, 4, color).unwrap();

        let descs = dev.descriptors.borrow();
        assert_eq!(
            u32::from_le_bytes(descs.get(&80).unwrap()[..4].try_into().unwrap()),
            color,
            "colour must be written at this device's directLEDColour offset"
        );
        assert_eq!(descs.get(&84).unwrap(), &[4]);
        // The 2i2's offsets must not be touched: 84 is this device's index
        // field (asserted above), and 88 is the start of its bulk array.
        assert!(
            !descs.contains_key(&OFF_DIRECT_LED_INDEX),
            "must not write the 2i2 index offset (88) — it is the bulk array here"
        );
        drop(descs);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8]);
    }

    /// Regression: restore read `selectedInput` at the 2i2's offset 331 on
    /// every model. A device without that control has an unrelated field
    /// there, and whatever byte it held decided the restore colour.
    #[test]
    fn restore_without_selected_input_never_reads_the_2i2_offset() {
        let dev = MockDevice::new();
        // Seed the 2i2 offset with the value that would mark input 0 selected.
        // A device with no selectedInput must ignore it.
        setup_device_with_selected_input(&dev, 0);

        let strategy = MuteStrategy {
            input_indices: vec![0, 1],
            number_leds: vec![4, 12],
            mute_colors: vec![],
            selected_color: 0x20FF_0000,
            unselected_color: 0x88FF_FF00,
            offsets: compact_layout_offsets(),
            extended: None,
        };

        clear_mute_indicator(&dev, &strategy).unwrap();

        // Both numbers restore to the unselected colour; neither is green.
        let descs = dev.descriptors.borrow();
        assert_eq!(
            u32::from_le_bytes(descs.get(&80).unwrap()[..4].try_into().unwrap()),
            strategy.unselected_color,
            "no selectedInput means no input is highlighted"
        );
        assert_eq!(descs.get(&84).unwrap(), &[12]);
        drop(descs);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8, 8]);
    }

    /// The 2i2 keeps its existing behaviour: the selected input goes green.
    #[test]
    fn restore_with_selected_input_still_highlights_the_selected_number() {
        let dev = MockDevice::new();
        setup_device_with_selected_input(&dev, 1);
        let strategy = make_strategy_both_inputs();

        clear_mute_indicator(&dev, &strategy).unwrap();

        // Input 2 (index 1) is selected and written last.
        let descs = dev.descriptors.borrow();
        assert_eq!(descs.get(&OFF_DIRECT_LED_INDEX).unwrap(), &[8]);
        assert_eq!(
            u32::from_le_bytes(
                descs.get(&OFF_DIRECT_LED_COLOUR).unwrap()[..4]
                    .try_into()
                    .unwrap()
            ),
            strategy.selected_color
        );
    }

    #[test]
    fn set_single_led_writes_colour_index_notify() {
        let dev = MockDevice::new();
        let color = 0xFF00_0000u32;
        set_single_led(&dev, &DeviceOffsets::default(), 0, color).unwrap();

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
        set_single_led(&dev, &DeviceOffsets::default(), 0, 0xFF00_0000).unwrap();

        let descs = dev.descriptors.borrow();
        assert!(!descs.contains_key(&OFF_ENABLE_DIRECT_LED));
        assert!(!descs.contains_key(&OFF_DIRECT_LED_VALUES));
    }

    #[test]
    fn solo_single_led_uses_solo_fields_without_2i2_value_field() {
        let dev = MockDevice::new();
        set_single_led(&dev, &solo_strategy(&[]).offsets, 7, 0x12_34_56_00).unwrap();

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
        let dev = MockDevice::new();
        let mut strategy = solo_strategy(&[0]);
        strategy.mute_colors.clear(); // exercise the global-colour fallback
        let color = 0xAA_BB_CC_00;
        apply_mute_indicator_with_mode(&dev, &strategy, color, IndicatorRenderMode::Auto).unwrap();

        let descriptors = dev.descriptors.borrow();
        assert!(!descriptors.contains_key(&72));
        assert!(!descriptors.contains_key(&88));
        assert_eq!(descriptors.get(&80), Some(&color.to_le_bytes().to_vec()));
        assert_eq!(descriptors.get(&84), Some(&vec![4]));
        drop(descriptors);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8]);

        clear_mute_indicator_with_mode(&dev, &strategy, IndicatorRenderMode::Auto).unwrap();
        let descriptors = dev.descriptors.borrow();
        assert_eq!(
            descriptors.get(&80),
            Some(&SOLO_WHITE.to_le_bytes().to_vec())
        );
        assert_eq!(descriptors.get(&84), Some(&vec![4]));
        drop(descriptors);
        assert_eq!(dev.notifies.borrow().as_slice(), &[8, 8]);
    }

    #[test]
    fn solo_second_input_and_legacy_halo_config_never_target_air_or_halos() {
        let dev = MockDevice::new();
        let mut strategy = solo_strategy(&[1]);
        strategy.mute_colors = vec![0x12_34_56_00];
        for mode in [
            IndicatorRenderMode::Auto,
            IndicatorRenderMode::Numbers,
            IndicatorRenderMode::NumbersBlink,
            IndicatorRenderMode::HalosSolid,
        ] {
            apply_mute_indicator_with_mode(&dev, &strategy, 0xFF00_0000, mode).unwrap();
            assert_eq!(dev.descriptors.borrow().get(&84), Some(&vec![12]));
            blank_mute_indicator(&dev, &strategy).unwrap();
            assert_eq!(dev.descriptors.borrow().get(&84), Some(&vec![12]));
            assert_eq!(dev.descriptors.borrow().get(&80), Some(&vec![0; 4]));
            clear_mute_indicator_with_mode(&dev, &strategy, mode).unwrap();
            assert_eq!(dev.descriptors.borrow().get(&84), Some(&vec![12]));
        }
        assert_eq!(dev.descriptors.borrow().len(), 2);
        assert!(dev.notifies.borrow().iter().all(|&event| event == 8));
        assert_eq!(strategy.input_indices, vec![1]);
    }

    /// `extended` lights both numbers and the Output LEDs whatever
    /// `mute_inputs` says, and restores all four to white.
    #[test]
    fn solo_extended_owns_both_numbers_and_the_output_indicator() {
        use crate::device::mock::MockOp;
        let dev = MockDevice::new();
        let strategy = solo_strategy(&[0]);
        apply_mute_indicator_with_mode(&dev, &strategy, 0xFF00_0000, IndicatorRenderMode::Extended)
            .unwrap();
        clear_mute_indicator_with_mode(&dev, &strategy, IndicatorRenderMode::Extended).unwrap();
        let indices: Vec<(u8, u32)> = dev
            .ops
            .borrow()
            .chunks(3)
            .map(|op| match op {
                [MockOp::Set(80, c), MockOp::Set(84, i), MockOp::Notify(8)] => {
                    (i[0], u32::from_le_bytes(c[..4].try_into().unwrap()))
                }
                other => panic!("unexpected ops {other:?}"),
            })
            .collect();
        assert_eq!(
            indices,
            vec![
                (4, 0xFF00_0000),
                (12, 0xFF00_0000),
                (24, 0xFF00_0000),
                (25, 0xFF00_0000),
                (4, SOLO_WHITE),
                (12, SOLO_WHITE),
                (24, SOLO_WHITE),
                (25, SOLO_WHITE),
            ]
        );
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
        let dev = MockDevice::new();
        let strategy = solo_strategy(&[]);
        assert_eq!(strategy.number_leds, vec![4, 12]);
        apply_mute_indicator(&dev, &strategy, 0xFF00_0000).unwrap();

        // MockDevice retains the final single-LED index; two notifications
        // prove both known Solo channel numbers were issued, with no halo write.
        assert_eq!(dev.descriptors.borrow().get(&84), Some(&vec![12]));
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
            offsets: Default::default(),
            extended: None,
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

        let result = set_single_led(&dev, &DeviceOffsets::default(), 0, 0xFF00_0000);
        assert!(result.is_err(), "should propagate set_descriptor error");
    }

    #[test]
    fn set_single_led_propagates_notify_error() {
        let dev = MockDevice::new();
        dev.fail_data_notify.set(true);

        let result = set_single_led(&dev, &DeviceOffsets::default(), 0, 0xFF00_0000);
        assert!(result.is_err(), "should propagate data_notify error");
    }
}
