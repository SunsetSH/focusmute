//! Golden device-write sequences for the Scarlett Solo and 2i2 4th Gen.
//!
//! The snapshot in `golden/led_ops.txt` was recorded on FocusMute 1.1.0,
//! before descriptor offsets moved into model profiles. It is the exact
//! sequence of descriptor reads, writes and notifies the hardware sees for
//! every indicator mode, input selection, Direct and LED Lab operation. A
//! diff here changes what the panel does, so it has to be deliberate.
//!
//! Regenerate with `FOCUSMUTE_BLESS=1 cargo test --test golden_led_ops`.

use std::fmt::Write as _;

use focusmute_lib::config::Config;
use focusmute_lib::context::DeviceContext;
use focusmute_lib::device::ScarlettDevice;
use focusmute_lib::device::mock::{MockDevice, MockOp};
use focusmute_lib::led::{self, IndicatorRenderMode};
use focusmute_lib::monitor::MuteIndicator;

const SNAPSHOT: &str = "tests/golden/led_ops.txt";

const MODES: [&str; 4] = ["numbers", "numbers_blink", "extended", "halos_solid"];
const INPUTS: [&str; 4] = ["all", "1", "2", "1,2"];

fn device(name: &str) -> MockDevice {
    let mut dev = MockDevice::new();
    dev.info_mut().device_name = name.into();
    dev.info_mut().firmware.major = 2;
    dev.info_mut().firmware.minor = 0;
    dev.info_mut().firmware.stage_release = 2417;
    dev.info_mut().firmware.build_nr = 0;
    dev
}

/// Firmware schemas read from real devices on 2.0.2417.0. A device with a
/// model profile never asks for its schema; one without does, and gets this.
const SOLO_SCHEMA: &str = include_str!("../../../docs/device_firmware_schema_solo_2.0.2417.0.json");
const SCHEMA_2I2: &str = include_str!("../../../docs/device_firmware_schema.json");

fn solo() -> MockDevice {
    let dev = device("Scarlett Solo 4th Gen-000171cf");
    dev.serve_schema(SOLO_SCHEMA);
    dev
}

fn scarlett_2i2() -> MockDevice {
    let dev = device("Scarlett 2i2 4th Gen-0003186a");
    dev.serve_schema(SCHEMA_2I2);
    dev
}

/// The context the tray resolves for a connected device on firmware 2.x.
fn context(dev: &MockDevice) -> DeviceContext {
    let dir = tempfile::tempdir().unwrap();
    DeviceContext::resolve_at(dev, false, Some(&dir.path().join("schema.json"))).unwrap()
}

fn indicator(ctx: &DeviceContext, mode: &str, inputs: &str, colours: bool) -> MuteIndicator {
    let mut config = Config::default();
    config.indicator.mute_inputs = inputs.into();
    config.indicator.mode = mode.into();
    if colours {
        config
            .indicator
            .input_colors
            .insert("1".into(), "#00FF00".into());
        config
            .indicator
            .input_colors
            .insert("2".into(), "#0000FF".into());
    }
    let (_, strategy, _) = led::resolve_strategy_from_config(
        &mut config,
        ctx.input_count(),
        ctx.profile,
        ctx.predicted.as_ref(),
        &ctx.offsets,
    )
    .unwrap();
    let mut indicator = MuteIndicator::new(2, false, led::mute_color_or_default(&config), strategy);
    indicator.set_render_mode(IndicatorRenderMode::from_config(mode));
    indicator
}

fn take_ops(dev: &MockDevice) -> String {
    let mut out = String::new();
    for op in dev.ops.borrow_mut().drain(..) {
        match op {
            MockOp::Get(offset, len) => writeln!(out, "  get {offset} {len}"),
            MockOp::Set(offset, bytes) => {
                let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
                writeln!(out, "  set {offset} {}", hex.join(" "))
            }
            MockOp::Notify(id) => writeln!(out, "  notify {id}"),
        }
        .unwrap();
    }
    out
}

fn section(out: &mut String, title: &str, dev: &MockDevice) {
    writeln!(out, "== {title}").unwrap();
    out.push_str(&take_ops(dev));
}

fn record_indicator(out: &mut String, model: &str, dev: &MockDevice) {
    let ctx = context(dev);
    dev.ops.borrow_mut().clear();
    for mode in MODES {
        for inputs in INPUTS {
            for colours in [false, true] {
                if colours && inputs != "all" {
                    continue;
                }
                let tag = format!(
                    "{model} {mode} inputs={inputs}{}",
                    if colours { " colours" } else { "" }
                );
                let ind = indicator(&ctx, mode, inputs, colours);
                ind.apply_mute(dev).unwrap();
                section(out, &format!("{tag} apply"), dev);
                led::blank_mute_indicator(dev, ind.strategy()).unwrap();
                section(out, &format!("{tag} blank"), dev);
                ind.clear_mute(dev).unwrap();
                section(out, &format!("{tag} clear"), dev);
                ind.restore(dev).unwrap();
                section(out, &format!("{tag} restore"), dev);
                led::restore_on_exit(dev, ind.strategy()).unwrap();
                section(out, &format!("{tag} restore_on_exit"), dev);
                led::refresh_after_reconnect(dev, ind.strategy(), ind.mute_color(), true).unwrap();
                section(out, &format!("{tag} refresh_after_reconnect"), dev);
            }
        }
    }
}

fn record() -> String {
    let mut out = String::new();

    let dev = solo();
    record_indicator(&mut out, "solo", &dev);

    let dev = scarlett_2i2();
    record_indicator(&mut out, "2i2 selected=0", &dev);
    let dev = scarlett_2i2();
    dev.set_descriptor(focusmute_lib::protocol::OFF_SELECTED_INPUT, &[1])
        .unwrap();
    record_indicator(&mut out, "2i2 selected=1", &dev);

    // Direct Monitor and the LED Lab touch Solo-only descriptor fields.
    for (model, dev) in [("solo", solo()), ("2i2", scarlett_2i2())] {
        let ctx = context(&dev);
        dev.ops.borrow_mut().clear();
        for enabled in [true, false] {
            let _ = direct_set(&dev, &ctx, enabled);
            section(&mut out, &format!("{model} direct set {enabled}"), &dev);
        }
        let state = direct_read(&dev, &ctx);
        writeln!(out, "== {model} direct read -> {state:?}").unwrap();
        out.push_str(&take_ops(&dev));

        let result = lab_test(&dev, &ctx, 4, 0xFF00_0000);
        writeln!(out, "== {model} lab test 4 -> {}", result.is_ok()).unwrap();
        out.push_str(&take_ops(&dev));
        let result = lab_test(&dev, &ctx, 0, 0xFF00_0000);
        writeln!(out, "== {model} lab test 0 -> {}", result.is_ok()).unwrap();
        out.push_str(&take_ops(&dev));
        let result = lab_restore(&dev, &ctx, &[0, 1]);
        writeln!(out, "== {model} lab restore -> {}", result.is_ok()).unwrap();
        out.push_str(&take_ops(&dev));
    }

    out
}

// Version seams: the Direct and LED Lab entry points change signature when
// their offsets move into the model profile. Only these adapters follow.

fn direct_set(dev: &MockDevice, _ctx: &DeviceContext, enabled: bool) -> Result<(), String> {
    match focusmute_lib::models::panel(dev.info()) {
        Some(panel) => focusmute_lib::direct::set_direct_enabled(dev, panel.controls, enabled)
            .map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

fn direct_read(dev: &MockDevice, _ctx: &DeviceContext) -> Option<bool> {
    let panel = focusmute_lib::models::panel(dev.info())?;
    focusmute_lib::direct::direct_enabled(dev, panel.controls).ok()
}

fn lab_test(dev: &MockDevice, _ctx: &DeviceContext, index: u8, color: u32) -> Result<(), String> {
    let panel = focusmute_lib::models::panel(dev.info());
    led::solo::test_led(dev, panel, index, color).map_err(|e| e.to_string())
}

fn lab_restore(dev: &MockDevice, _ctx: &DeviceContext, inputs: &[usize]) -> Result<(), String> {
    let panel = focusmute_lib::models::panel(dev.info());
    led::solo::restore_test(dev, panel, inputs).map_err(|e| e.to_string())
}

#[test]
fn device_writes_match_the_recorded_snapshot() {
    let actual = record();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT);
    if std::env::var_os("FOCUSMUTE_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with FOCUSMUTE_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    if actual != expected {
        let first = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()));
        let context = |s: &str| {
            s.lines()
                .skip(first.saturating_sub(6))
                .take(12)
                .collect::<Vec<_>>()
                .join("\n")
        };
        panic!(
            "device writes changed at line {}\n--- expected\n{}\n--- actual\n{}",
            first + 1,
            context(&expected),
            context(&actual)
        );
    }
}
