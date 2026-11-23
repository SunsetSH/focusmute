//! A Scarlett Solo on firmware 3.x.
//!
//! No Solo 3.x schema has been read yet. Firmware 3.0.2452.0 on the 2i2 moved
//! the version block from 4 to 20, the name from 16 to 40 and every member
//! after the name by 24 bytes; these tests model a Solo with the same shift
//! and check that nothing is written at a 2.x offset.

use focusmute_lib::config::Config;
use focusmute_lib::context::DeviceContext;
use focusmute_lib::device::mock::{MockDevice, MockOp};
use focusmute_lib::device::{DeviceInfo, HeaderLayout, ScarlettDevice};
use focusmute_lib::led::{self, IndicatorRenderMode};
use focusmute_lib::monitor::MuteIndicator;

const SOLO_SCHEMA: &str = include_str!("../../../docs/device_firmware_schema_solo_2.0.2417.0.json");

/// The Solo 2.x schema with the 3.x shift applied to everything after `name`.
fn shifted_solo_schema() -> String {
    let mut root: serde_json::Value = serde_json::from_str(SOLO_SCHEMA).unwrap();
    let members = root
        .pointer_mut("/structs/APP_SPACE/members")
        .and_then(|m| m.as_object_mut())
        .unwrap();
    for member in members.values_mut() {
        let offset = member["offset"].as_u64().unwrap();
        if offset >= 48 {
            member["offset"] = (offset + 24).into();
        }
    }
    root.to_string()
}

fn solo_on_v3() -> MockDevice {
    let mut dev = MockDevice::new();
    dev.info_mut().device_name = "Scarlett Solo 4th Gen-000171cf".into();
    dev.info_mut().header_layout = HeaderLayout::V3;
    dev.info_mut().firmware.major = 3;
    dev
}

fn context(dev: &MockDevice) -> focusmute_lib::error::Result<DeviceContext> {
    let dir = tempfile::tempdir().unwrap();
    DeviceContext::resolve_at(dev, false, Some(&dir.path().join("schema.json")))
}

/// Descriptor offsets the Solo uses on firmware 2.x. With the 3.x shift each
/// of them lands on some other member.
const V2_ONLY: [u32; 6] = [72, 80, 84, 216, 217, 264];

#[test]
fn a_solo_header_on_firmware_3_is_recognised() {
    let mut hdr = [0u8; 72];
    hdr[20..22].copy_from_slice(&3u16.to_le_bytes());
    hdr[24..28].copy_from_slice(&2452u32.to_le_bytes());
    let name = b"Scarlett Solo 4th Gen-000171cf";
    hdr[40..40 + name.len()].copy_from_slice(name);

    let mut info = DeviceInfo {
        path: String::new(),
        config_raw: vec![],
        init_raw: vec![],
        device_name: String::new(),
        firmware: Default::default(),
        header_layout: HeaderLayout::Unknown,
        serial: None,
    };
    info.apply_header(&hdr);
    assert_eq!(info.header_layout, HeaderLayout::V3);
    assert_eq!(info.model(), "Scarlett Solo 4th Gen");
    assert_eq!(info.firmware.to_string(), "3.0.2452.0");
}

#[test]
fn indication_writes_only_at_the_schema_offsets() {
    let dev = solo_on_v3();
    dev.serve_schema(&shifted_solo_schema());
    let ctx = context(&dev).unwrap();
    assert!(ctx.profile.is_some(), "the profile still names the LEDs");
    assert_eq!(ctx.offsets.direct_led_colour, 104);
    assert_eq!(ctx.offsets.direct_led_index, 108);
    assert_eq!(ctx.offsets.enable_direct_led, 96);

    for mode in ["numbers", "numbers_blink", "extended"] {
        let mut config = Config::default();
        config.indicator.mode = mode.into();
        let (_, strategy, _) = led::resolve_strategy_from_config(
            &mut config,
            ctx.input_count(),
            ctx.profile,
            ctx.predicted.as_ref(),
            &ctx.offsets,
        )
        .unwrap();
        let mut ind = MuteIndicator::new(2, false, 0xFF00_0000, strategy);
        ind.set_render_mode(IndicatorRenderMode::from_config(mode));
        dev.ops.borrow_mut().clear();
        ind.apply_mute(&dev).unwrap();
        led::blank_mute_indicator(&dev, ind.strategy()).unwrap();
        ind.clear_mute(&dev).unwrap();

        let ops = dev.ops.borrow();
        assert!(!ops.is_empty(), "{mode}: the indicator must still work");
        for op in ops.iter() {
            match op {
                MockOp::Set(offset, _) => {
                    assert!(matches!(offset, 104 | 108), "{mode}: write at {offset}")
                }
                MockOp::Notify(id) => assert_eq!(*id, 8, "{mode}"),
                MockOp::Get(offset, _) => panic!("{mode}: unexpected read at {offset}"),
            }
        }
    }
}

#[test]
fn direct_and_the_led_lab_are_withheld() {
    let dev = solo_on_v3();
    assert!(focusmute_lib::models::panel(dev.info()).is_none());
    dev.ops.borrow_mut().clear();
    assert!(led::solo::test_led(&dev, None, 4, 0xFF00_0000).is_err());
    assert!(led::solo::restore_test(&dev, None, &[0, 1]).is_err());
    assert!(dev.ops.borrow().is_empty());
}

#[test]
fn without_a_schema_the_device_is_refused_and_untouched() {
    for layout in [HeaderLayout::V3, HeaderLayout::Unknown] {
        let mut dev = solo_on_v3();
        dev.info_mut().header_layout = layout;
        let err = context(&dev).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported device"),
            "{layout:?}: {err}"
        );
        let ops = dev.ops.borrow();
        assert!(
            ops.iter()
                .all(|op| !matches!(op, MockOp::Set(..) | MockOp::Notify(..))),
            "{layout:?}: {ops:?}"
        );
        assert!(
            ops.iter()
                .all(|op| !matches!(op, MockOp::Get(o, _) if V2_ONLY.contains(o))),
            "{layout:?}: {ops:?}"
        );
    }
}
