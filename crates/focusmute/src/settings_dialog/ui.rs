//! Cross-platform egui settings dialog.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use focusmute_lib::config::Config;
use focusmute_lib::device::{PlatformDevice, ScarlettDevice, open_device_by_serial};
use focusmute_lib::led;
use focusmute_lib::meter;
use global_hotkey::{GlobalHotKeyEvent, HotKeyState, hotkey::HotKey};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use super::{
    MAX_SOUND_FILE_BYTES, SettingsToggleCallback, SoundPreviewPlayer, combo_to_mute_inputs,
    inputs_combo_items,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureTarget {
    Toggle,
    PushToTalk,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Main,
    Advanced,
    LedLab,
    About,
}

/// Tracks which side of the color sync last changed.
#[derive(PartialEq)]
pub(crate) enum ColorDirty {
    Neither,
    Text,
    Picker,
}

/// Muted-talk blink sensitivity presets shown in the settings dialog,
/// mapped to raw `talk_threshold` meter values (higher sensitivity =
/// lower threshold). The raw `[indicator].talk_threshold` TOML key
/// remains the escape hatch for setups outside these presets.
const TALK_SENSITIVITY_PRESETS: &[(&str, u32)] = &[("Low", 500), ("Medium", 250), ("High", 100)];

/// Display text for the sensitivity combo: the preset name when the current
/// threshold matches one, otherwise "Custom (n)" — a hand-edited TOML value
/// is shown as-is and never silently replaced.
fn sensitivity_text(threshold: u32) -> String {
    TALK_SENSITIVITY_PRESETS
        .iter()
        .find(|(_, v)| *v == threshold)
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| format!("Custom ({threshold})"))
}

pub struct SettingsApp {
    // ── Form state ──
    color_text: String,
    color_rgb: [f32; 3],
    color_dirty: ColorDirty,

    hotkey: String,
    ptt_hotkey: String,
    capturing: Option<CaptureTarget>,

    indicator_mode: String,
    language: String,
    direct_button_enabled: bool,
    selected_tab: SettingsTab,
    is_solo: bool,

    mute_inputs_index: usize,
    mute_inputs_items: Vec<String>,
    input_count: usize,

    blink_on_talk: bool,
    blink_while_muted: bool,
    talk_threshold: u32,
    meter_device: Option<PlatformDevice>,
    meter_levels: Vec<u32>,
    meter_error: Option<String>,
    next_meter_read: Instant,
    led_lab_index: u8,
    led_lab_result: String,
    led_lab_touched: bool,

    // The hotkey remains registered by the tray while this modal UI owns the
    // main thread. Poll its shared receiver here so mute still works.
    registered_toggle_id: Option<u32>,
    toggle_callback: Option<SettingsToggleCallback>,

    sound_enabled: bool,
    suppress_browser_sync_sound: bool,
    mute_sound_volume: f32,
    unmute_sound_volume: f32,
    autostart: bool,
    notifications_enabled: bool,
    log_level: String,

    mute_sound_path: String,
    unmute_sound_path: String,

    on_mute_url: String,
    on_unmute_url: String,
    on_mute_body: String,
    on_unmute_body: String,

    browser_sync_port: String,
    browser_sync_reverse: bool,

    // ── Sound preview ──
    preview_player: SoundPreviewPlayer,

    // ── Non-editable fields carried through ──
    original: Config,

    // ── About section (read-only) ──
    device_lines: Vec<(String, String)>,

    // ── Validation ──
    errors: Vec<String>,
    apply_message: Option<String>,

    // ── Shared result (read by caller after run_native returns) ──
    result: Arc<Mutex<Option<Config>>>,

    /// Resize the viewport on the next frame.
    needs_resize: bool,
}

#[cfg(windows)]
fn capture_pressed_hotkey(_ctx: &egui::Context) -> Option<Option<String>> {
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    let down = |key| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 != 0 };
    let pressed = |key| unsafe { GetAsyncKeyState(key) as u16 & 1 != 0 };
    let key_name = |key: i32| match key {
        0x13 => Some("Pause".to_owned()),
        0x20 => Some("Space".to_owned()),
        0x21 => Some("PageDown".to_owned()),
        0x22 => Some("PageUp".to_owned()),
        0x23 => Some("End".to_owned()),
        0x24 => Some("Home".to_owned()),
        0x25 => Some("Left".to_owned()),
        0x26 => Some("Up".to_owned()),
        0x27 => Some("Right".to_owned()),
        0x28 => Some("Down".to_owned()),
        0x2D => Some("Insert".to_owned()),
        0x2E => Some("Delete".to_owned()),
        0x70..=0x87 => Some(format!("F{}", key - 0x6F)),
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(key as u32).map(|c| c.to_string()),
        _ => None,
    };
    for key in 0x08..=0xFE {
        if matches!(key, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN) || !pressed(key) {
            continue;
        }
        let Some(key) = key_name(key) else { continue };
        let mut parts = Vec::new();
        if down(VK_CONTROL) {
            parts.push("Ctrl");
        }
        if down(VK_MENU) {
            parts.push("Alt");
        }
        if down(VK_SHIFT) {
            parts.push("Shift");
        }
        if down(VK_LWIN) || down(VK_RWIN) {
            parts.push("Super");
        }
        parts.push(&key);
        return Some(Some(parts.join("+")));
    }
    None
}

#[cfg(not(windows))]
fn capture_pressed_hotkey(ctx: &egui::Context) -> Option<Option<String>> {
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => {
                let raw = format!("{key:?}");
                if matches!(
                    raw.as_str(),
                    "ShiftLeft"
                        | "ShiftRight"
                        | "ControlLeft"
                        | "ControlRight"
                        | "AltLeft"
                        | "AltRight"
                        | "SuperLeft"
                        | "SuperRight"
                ) {
                    return None;
                }
                if raw == "Escape" {
                    return Some(None);
                }
                let mut parts = Vec::new();
                if modifiers.ctrl || modifiers.command {
                    parts.push("Ctrl");
                }
                if modifiers.alt {
                    parts.push("Alt");
                }
                if modifiers.shift {
                    parts.push("Shift");
                }
                parts.push(&raw);
                Some(Some(parts.join("+")))
            }
            _ => None,
        })
    })
}
impl SettingsApp {
    /// Capture a shortcut using the native key state on Windows. egui exposes
    /// only a limited logical-key set (notably it omits Pause), while the
    /// hotkey parser accepts Win32 virtual keys.
    fn capture_hotkey(&mut self, ctx: &egui::Context) {
        let Some(target) = self.capturing else {
            return;
        };
        let captured = capture_pressed_hotkey(ctx);
        if let Some(value) = captured {
            if let Some(value) = value {
                match target {
                    CaptureTarget::Toggle => self.hotkey = value,
                    CaptureTarget::PushToTalk => self.ptt_hotkey = value,
                }
            }
            self.capturing = None;
        }
    }
    fn forward_registered_toggle_hotkey(&self) {
        let Some(toggle_id) = self.registered_toggle_id else {
            return;
        };
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if self.capturing.is_none()
                && event.id == toggle_id
                && event.state == HotKeyState::Pressed
                && let Some(callback) = &self.toggle_callback
            {
                callback();
            }
        }
    }

    pub fn new(
        config: Config,
        input_count: usize,
        is_solo: bool,
        device_lines: Vec<(String, String)>,
        result: Arc<Mutex<Option<Config>>>,
        toggle_callback: Option<SettingsToggleCallback>,
        cc: &eframe::CreationContext<'_>,
    ) -> Self {
        // Apply widget style customizations
        let mut style = (*cc.egui_ctx.global_style()).clone();
        let corner_radius = egui::CornerRadius::same(4);
        style.visuals.widgets.noninteractive.corner_radius = corner_radius;
        style.visuals.widgets.inactive.corner_radius = corner_radius;
        style.visuals.widgets.active.corner_radius = corner_radius;
        style.visuals.widgets.hovered.corner_radius = corner_radius;
        cc.egui_ctx.set_global_style(style);

        let color_rgb = led::parse_color(&config.indicator.mute_color)
            .ok()
            .map(led::color_to_rgb)
            .unwrap_or([1.0, 0.0, 0.0]);
        let registered_toggle_id = config
            .keyboard
            .hotkey
            .parse::<HotKey>()
            .ok()
            .map(|hotkey| hotkey.id());
        let (mut mute_inputs_items, mute_inputs_index) = inputs_combo_items(&config, input_count);
        if is_solo && input_count == 2 {
            mute_inputs_items = if config.system.language.eq_ignore_ascii_case("ru") {
                vec![
                    "Все".into(),
                    "Вход 1 — инструмент".into(),
                    "Вход 2 — микрофон".into(),
                    "Входы 1 + 2".into(),
                ]
            } else {
                vec![
                    "All".into(),
                    "Input 1 — instrument".into(),
                    "Input 2 — microphone".into(),
                    "Inputs 1 + 2".into(),
                ]
            };
        }

        Self {
            color_text: config.indicator.mute_color.clone(),
            color_rgb,
            color_dirty: ColorDirty::Neither,

            hotkey: config.keyboard.hotkey.clone(),
            ptt_hotkey: config.keyboard.push_to_talk_hotkey.clone(),
            capturing: None,
            indicator_mode: if matches!(
                config.indicator.mode.as_str(),
                "halos_solid" | "numbers_blink"
            ) {
                "numbers".into()
            } else {
                config.indicator.mode.clone()
            },
            language: config.system.language.clone(),
            direct_button_enabled: config.hardware_button.direct_button_enabled,
            selected_tab: SettingsTab::Main,
            is_solo,

            mute_inputs_index,
            mute_inputs_items,
            input_count,

            blink_on_talk: config.indicator.blink_on_talk,
            blink_while_muted: config.indicator.blink_while_muted
                || config.indicator.mode == "numbers_blink",
            talk_threshold: config.indicator.talk_threshold,
            meter_device: open_device_by_serial(&config.system.device_serial).ok(),
            meter_levels: Vec::new(),
            meter_error: None,
            next_meter_read: Instant::now(),
            led_lab_index: 4,
            led_lab_result: String::new(),
            led_lab_touched: false,
            registered_toggle_id,
            toggle_callback,

            sound_enabled: config.sound.sound_enabled,
            suppress_browser_sync_sound: config.sound.suppress_browser_sync_sound,
            mute_sound_volume: config.sound.mute_sound_volume,
            unmute_sound_volume: config.sound.unmute_sound_volume,
            autostart: config.system.autostart,
            notifications_enabled: config.system.notifications_enabled,
            log_level: config.system.log_level.clone(),

            mute_sound_path: config.sound.mute_sound_path.clone(),
            unmute_sound_path: config.sound.unmute_sound_path.clone(),

            on_mute_url: config.hooks.on_mute_url.clone(),
            on_unmute_url: config.hooks.on_unmute_url.clone(),
            on_mute_body: config.hooks.on_mute_body.clone(),
            on_unmute_body: config.hooks.on_unmute_body.clone(),

            browser_sync_port: config.system.browser_sync_port.to_string(),
            browser_sync_reverse: config.system.browser_sync_reverse,

            preview_player: SoundPreviewPlayer::new(),

            original: config,

            device_lines,

            errors: Vec::new(),
            apply_message: None,

            result,

            needs_resize: true,
        }
    }

    fn validated_config(&self) -> Result<Config, Vec<String>> {
        build_and_validate_config(&ValidateParams {
            color_dirty: &self.color_dirty,
            color_text: &self.color_text,
            color_rgb: self.color_rgb,
            hotkey: &self.hotkey,
            ptt_hotkey: &self.ptt_hotkey,
            indicator_mode: &self.indicator_mode,
            language: &self.language,
            direct_button_enabled: self.direct_button_enabled,
            sound_enabled: self.sound_enabled,
            suppress_browser_sync_sound: self.suppress_browser_sync_sound,
            mute_sound_volume: self.mute_sound_volume,
            unmute_sound_volume: self.unmute_sound_volume,
            autostart: self.autostart,
            notifications_enabled: self.notifications_enabled,
            log_level: &self.log_level,
            mute_inputs_index: self.mute_inputs_index,
            input_count: self.input_count,
            mute_sound_path: &self.mute_sound_path,
            unmute_sound_path: &self.unmute_sound_path,
            on_mute_url: &self.on_mute_url,
            on_unmute_url: &self.on_unmute_url,
            on_mute_body: &self.on_mute_body,
            on_unmute_body: &self.on_unmute_body,
            browser_sync_port: &self.browser_sync_port,
            browser_sync_reverse: self.browser_sync_reverse,
            blink_on_talk: self.blink_on_talk,
            blink_while_muted: self.blink_while_muted,
            talk_threshold: self.talk_threshold,
            original: &self.original,
            max_sound_bytes: MAX_SOUND_FILE_BYTES,
        })
    }

    /// Save the edited configuration and close the dialog.
    fn try_save(&mut self, ctx: &egui::Context) {
        match self.validated_config() {
            Ok(config) => {
                *self.result.lock().unwrap() = Some(config);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(errs) => {
                self.errors = errs;
            }
        }
    }

    /// Persist without closing. The tray applies the returned configuration
    /// when this modal window closes; retaining it here makes Apply useful for
    /// keeping a stable TOML file while continuing to edit other tabs.
    fn try_apply(&mut self) {
        match self.validated_config() {
            Ok(config) => match config.save() {
                Ok(()) => {
                    self.original = config.clone();
                    *self.result.lock().unwrap() = Some(config);
                    self.apply_message = Some(
                        if self.language.eq_ignore_ascii_case("ru") {
                            "Настройки сохранены. Они начнут действовать после закрытия окна."
                        } else {
                            "Saved. Changes apply when this window closes."
                        }
                        .into(),
                    );
                    self.errors.clear();
                }
                Err(error) => self.errors = vec![format!("Could not save settings: {error}")],
            },
            Err(errs) => self.errors = errs,
        }
    }

    fn refresh_meter(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        if now < self.next_meter_read {
            return;
        }
        self.next_meter_read = now + Duration::from_millis(90);
        if self.meter_device.is_none() {
            self.meter_device = open_device_by_serial(&self.original.system.device_serial).ok();
        }
        if let Some(device) = self.meter_device.as_ref() {
            match meter::read_meters(device, meter::METER_COUNT) {
                Ok(levels) => {
                    self.meter_levels = levels;
                    self.meter_error = None;
                }
                Err(error) => {
                    self.meter_error = Some(error.to_string());
                    self.meter_device = None;
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(90));
    }

    fn led_lab_device(&mut self) -> Result<&PlatformDevice, String> {
        if self.meter_device.is_none() {
            self.meter_device = open_device_by_serial(&self.original.system.device_serial).ok();
        }
        let device = self
            .meter_device
            .as_ref()
            .ok_or_else(|| "The Scarlett device is not available.".to_string())?;
        if !focusmute_lib::direct::is_solo(device) {
            return Err("LED Lab is available only for Scarlett Solo 4th Gen.".into());
        }
        Ok(device)
    }

    fn led_lab_reset(&mut self) {
        let result = (|| {
            let inputs = self.active_input_indices();
            let device = self.led_lab_device()?;
            led::solo::restore_test(device, &inputs).map_err(|e| e.to_string())?;
            Ok::<_, String>(
                "Selected channel numbers restored to white; DATA_NOTIFY(8). Bulk array unchanged."
                    .into(),
            )
        })();
        self.led_lab_result = result.unwrap_or_else(|error| format!("Error: {error}"));
    }

    fn led_lab_single(&mut self) {
        // Mark before I/O: a partially completed command may still need cleanup.
        self.led_lab_touched |= matches!(
            self.led_lab_index,
            led::solo::INPUT_1_LED | led::solo::INPUT_2_LED
        );
        let result = (|| {
            let color = led::parse_color(&self.color_text).map_err(|e| e.to_string())?;
            let index = self.led_lab_index;
            let device = self.led_lab_device()?;
            led::solo::test_led(device, index, color).map_err(|e| e.to_string())?;
            Ok::<_, String>(format!(
                "Single LED: color=80, index=84 ({index}), DATA_NOTIFY(8), color={color:#010X}."
            ))
        })();
        self.led_lab_result = result.unwrap_or_else(|error| format!("Error: {error}"));
    }

    fn led_lab_snapshot(&mut self) {
        let result = (|| {
            let device = self.led_lab_device()?;
            let mode = device.get_descriptor(72, 1).map_err(|e| e.to_string())?;
            let direct = device.get_descriptor(264, 1).map_err(|e| e.to_string())?;
            let values = device.get_descriptor(88, 128).map_err(|e| e.to_string())?;
            let air = device.get_descriptor(62, 1).map_err(|e| e.to_string())?;
            let mut text = format!(
                "READ ONLY: mode[72]={mode:02X?}; Air[62]={air:02X?}; Direct[264]={direct:02X?}\nCommand buffer (NOT measured panel colours), slots[88]:"
            );
            for (index, raw) in values.chunks_exact(4).enumerate() {
                let color = u32::from_le_bytes(raw.try_into().map_err(|_| "invalid slot data")?);
                write!(&mut text, " {index}={color:#010X}").map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(text)
        })();
        self.led_lab_result = result.unwrap_or_else(|error| format!("Error: {error}"));
    }

    fn active_input_indices(&self) -> Vec<usize> {
        if self.input_count == 0 || self.mute_inputs_index == 0 {
            return (0..self.input_count).collect();
        }
        if self.input_count >= 2 && self.mute_inputs_index == self.input_count + 1 {
            return (0..self.input_count).collect();
        }
        vec![self.mute_inputs_index.saturating_sub(1)]
    }

    fn cancel(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Snapshot all form fields for change detection (used to clear stale errors).
    fn form_snapshot(&self) -> FormSnapshot {
        FormSnapshot {
            color_text: self.color_text.clone(),
            color_rgb: self.color_rgb,
            hotkey: self.hotkey.clone(),
            ptt_hotkey: self.ptt_hotkey.clone(),
            indicator_mode: self.indicator_mode.clone(),
            language: self.language.clone(),
            direct_button_enabled: self.direct_button_enabled,
            mute_inputs_index: self.mute_inputs_index,
            sound_enabled: self.sound_enabled,
            suppress_browser_sync_sound: self.suppress_browser_sync_sound,
            mute_sound_volume: self.mute_sound_volume,
            unmute_sound_volume: self.unmute_sound_volume,
            autostart: self.autostart,
            notifications_enabled: self.notifications_enabled,
            log_level: self.log_level.clone(),
            mute_sound_path: self.mute_sound_path.clone(),
            unmute_sound_path: self.unmute_sound_path.clone(),
            on_mute_url: self.on_mute_url.clone(),
            on_unmute_url: self.on_unmute_url.clone(),
            on_mute_body: self.on_mute_body.clone(),
            on_unmute_body: self.on_unmute_body.clone(),
            browser_sync_port: self.browser_sync_port.clone(),
            browser_sync_reverse: self.browser_sync_reverse,
            blink_on_talk: self.blink_on_talk,
            blink_while_muted: self.blink_while_muted,
            talk_threshold: self.talk_threshold,
        }
    }
}

/// All form fields snapshotted for change detection — no element-count limit.
#[derive(PartialEq)]
struct FormSnapshot {
    color_text: String,
    color_rgb: [f32; 3],
    hotkey: String,
    ptt_hotkey: String,
    indicator_mode: String,
    language: String,
    direct_button_enabled: bool,
    mute_inputs_index: usize,
    sound_enabled: bool,
    suppress_browser_sync_sound: bool,
    mute_sound_volume: f32,
    unmute_sound_volume: f32,
    autostart: bool,
    notifications_enabled: bool,
    log_level: String,
    mute_sound_path: String,
    unmute_sound_path: String,
    on_mute_url: String,
    on_unmute_url: String,
    on_mute_body: String,
    on_unmute_body: String,
    browser_sync_port: String,
    browser_sync_reverse: bool,
    blink_on_talk: bool,
    blink_while_muted: bool,
    talk_threshold: u32,
}

impl Drop for SettingsApp {
    fn drop(&mut self) {
        if self.led_lab_touched
            && let Some(device) = &self.meter_device
            && let Err(error) = led::solo::restore_test(device, &self.active_input_indices())
        {
            log::warn!("[led-lab] could not restore test indicator: {error}");
        }
    }
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 0.34: the root Ui replaces the Context parameter. Keep a
        // `ctx` binding so viewport commands and &Context helpers read as before.
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let language = self.language.clone();
        let tr = |key| crate::i18n::tr(&language, key);
        self.forward_registered_toggle_hotkey();
        self.capture_hotkey(ctx);
        ctx.request_repaint_after(Duration::from_millis(30));
        // Height of the button area below content (separator + padding + buttons).
        const BUTTON_AREA_HEIGHT: f32 = 54.0;

        // Snapshot form state before rendering — if anything changes,
        // clear stale validation errors so the Save button stays reachable.
        let form_snap = self.form_snapshot();

        let mut content_bottom = 0.0_f32;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.selected_tab, SettingsTab::Main, tr("main"));
                ui.selectable_value(
                    &mut self.selected_tab,
                    SettingsTab::Advanced,
                    tr("advanced"),
                );
                ui.selectable_value(&mut self.selected_tab, SettingsTab::About, tr("about"));
            });
            ui.separator();

            if self.selected_tab == SettingsTab::Main {
            // ── Mute Indicator section ──
            section_frame(ui, tr("mute_indicator"), |ui| {
                egui::Grid::new("mute_indicator_grid")
                    .num_columns(2)
                    .min_col_width(80.0)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        // Mute Inputs row
                        ui.label(tr("mute_inputs"))
                            .on_hover_text("Which input LEDs show the mute color");
                        let selected_text = self
                            .mute_inputs_items
                            .get(self.mute_inputs_index)
                            .cloned()
                            .unwrap_or_default();
                        egui::ComboBox::from_id_salt("mute_inputs_combo")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for (i, item) in self.mute_inputs_items.iter().enumerate() {
                                    ui.selectable_value(&mut self.mute_inputs_index, i, item);
                                }
                            });
                        ui.end_row();

                        ui.label(tr("mute_display")).on_hover_text(
                            tr("mute_display_tip"),
                        );
                        egui::ComboBox::from_id_salt("indicator_mode_combo")
                            .selected_text(tr(if self.indicator_mode == "extended" { "extended" } else if self.indicator_mode == "numbers" { "numbers" } else { "auto" }))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.indicator_mode, "auto".to_string(), tr("auto"));
                                ui.selectable_value(&mut self.indicator_mode, "numbers".to_string(), tr("numbers"));
                                ui.selectable_value(&mut self.indicator_mode, "extended".to_string(), tr("extended"));
                            });
                        ui.end_row();

                        // Color row
                        ui.label(tr("mute_color"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let before = self.color_rgb;
                            ui.color_edit_button_rgb(&mut self.color_rgb);
                            if self.color_rgb != before {
                                self.color_dirty = ColorDirty::Picker;
                                self.color_text = led::rgb_to_hex(self.color_rgb);
                            }

                            let text_response = ui.add(
                                egui::TextEdit::singleline(&mut self.color_text)
                                    .desired_width(ui.available_width())
                                    .hint_text("#FF0000 or red"),
                            );
                            if text_response.changed() {
                                self.color_dirty = ColorDirty::Text;
                                if let Ok(val) = led::parse_color(&self.color_text) {
                                    self.color_rgb = led::color_to_rgb(val);
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(tr("blink_while_muted"));
                        let changed = ui.add_enabled(!self.blink_on_talk, egui::Checkbox::new(&mut self.blink_while_muted, "")).changed();
                        if changed && self.blink_while_muted { self.blink_on_talk = false; }
                        ui.end_row();

                        // Blink-on-talk row
                        ui.label(tr("blink_on_talk")).on_hover_text(
                            tr("blink_tip"),
                        );
                        let changed = ui.add_enabled(!self.blink_while_muted, egui::Checkbox::new(&mut self.blink_on_talk, "")).changed();
                        if changed && self.blink_on_talk { self.blink_while_muted = false; }
                        ui.end_row();

                        if self.blink_on_talk && !self.blink_while_muted {
                            ui.label(tr("sensitivity")).on_hover_text(
                                "How loud you need to be for the blink to trigger",
                            );
                            egui::ComboBox::from_id_salt("talk_sensitivity_combo")
                                .selected_text(sensitivity_text(self.talk_threshold))
                                .show_ui(ui, |ui| {
                                    for &(name, value) in TALK_SENSITIVITY_PRESETS {
                                        ui.selectable_value(
                                            &mut self.talk_threshold,
                                            value,
                                            name,
                                        );
                                    }
                                });
                            ui.end_row();
                        }
                    });
                if self.is_solo {
                    ui.add(egui::Label::new(tr("solo_number_note")).wrap());
                }
                self.refresh_meter(ctx);
                ui.add_space(6.0);
                ui.label(tr("input_levels")).on_hover_text(tr("input_level_tip"));
                if self.meter_levels.is_empty() {
                    ui.small(tr("meter_unavailable"));
                } else {
                    for input in self.active_input_indices() {
                        let level = self.meter_levels.get(input).copied().unwrap_or(0);
                        let fraction = (level as f32 / meter::METER_MAX as f32).clamp(0.0, 1.0);
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .text(format!("Input {}: {}", input + 1, level)),
                        );
                    }
                }
            });

            // ── Keyboard section ──
            section_frame(ui, tr("keyboard"), |ui| {
                let text_width = (ui.available_width() - 128.0).max(120.0);
                egui::Grid::new("hotkey_grid")
                    .num_columns(2)
                    .min_col_width(80.0)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr("hotkey"));
                        ui.horizontal(|ui| {
                            let response = ui.add(egui::TextEdit::singleline(&mut self.hotkey).desired_width(text_width - 72.0).hint_text("e.g. Ctrl+Shift+M"));
                            if response.clicked() {
                                self.capturing = Some(CaptureTarget::Toggle);
                            }
                            if ui.button(if self.capturing == Some(CaptureTarget::Toggle) { tr("press_keys") } else { tr("record") }).clicked() {
                                self.capturing = Some(CaptureTarget::Toggle);
                            }
                        });
                        ui.end_row();

                        ui.label(tr("push_to_talk"));
                        ui.horizontal(|ui| {
                            let response = ui.add(egui::TextEdit::singleline(&mut self.ptt_hotkey).desired_width(text_width - 72.0).hint_text("e.g. Ctrl+Space (empty = off)"));
                            if response.clicked() {
                                self.capturing = Some(CaptureTarget::PushToTalk);
                            }
                            if ui.button(if self.capturing == Some(CaptureTarget::PushToTalk) { tr("press_keys") } else { tr("record") }).clicked() {
                                self.capturing = Some(CaptureTarget::PushToTalk);
                            }
                        });
                        ui.end_row();
                    });
                if self.capturing.is_some() {
                    ui.label(egui::RichText::new(tr("press_keys")).italics());
                }
            });

            // ── Sound section ──
            section_frame(ui, tr("sound"), |ui| {
                ui.checkbox(&mut self.sound_enabled, tr("sound_feedback"));
                ui.add_space(4.0);

                // Keep controls inside the second grid column; Grid measures child desired sizes.
                let sound_control_width = (ui.available_width() - 128.0).max(120.0);

                // Reserve for the longer of the localized Browse and Play labels.
                let action_text_width = ui.fonts_mut(|f| {
                    [tr("browse"), tr("play")]
                        .iter()
                        .map(|label| {
                            f.layout_no_wrap(
                                (*label).into(),
                                egui::TextStyle::Button.resolve(ui.style()),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x
                        })
                        .fold(0.0_f32, f32::max)
                });
                let browse_btn_width = (action_text_width + ui.spacing().button_padding.x * 2.0)
                    .max(ui.spacing().interact_size.x);

                egui::Grid::new("sound_grid")
                    .num_columns(2)
                    .min_col_width(80.0)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr("mute_sound"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !self.mute_sound_path.is_empty() && ui.button(tr("clear")).clicked() {
                                self.mute_sound_path.clear();
                            }
                            if ui.button(tr("browse")).clicked()
                                && let Some(path) = browse_wav_file()
                            {
                                self.mute_sound_path = path;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut self.mute_sound_path)
                                    .desired_width((sound_control_width - browse_btn_width - 8.0).max(80.0))
                                    .hint_text(tr("built_in")),
                            );
                        });
                        ui.end_row();

                        volume_row(
                            ui,
                            browse_btn_width,
                            sound_control_width,
                            &language,
                            &mut self.mute_sound_volume,
                            &self.mute_sound_path,
                            crate::sound::SOUND_MUTED,
                            &mut self.preview_player,
                        );

                        ui.label(tr("unmute_sound"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !self.unmute_sound_path.is_empty() && ui.button(tr("clear")).clicked() {
                                self.unmute_sound_path.clear();
                            }
                            if ui.button(tr("browse")).clicked()
                                && let Some(path) = browse_wav_file()
                            {
                                self.unmute_sound_path = path;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut self.unmute_sound_path)
                                    .desired_width((sound_control_width - browse_btn_width - 8.0).max(80.0))
                                    .hint_text(tr("built_in")),
                            );
                        });
                        ui.end_row();

                        volume_row(
                            ui,
                            browse_btn_width,
                            sound_control_width,
                            &language,
                            &mut self.unmute_sound_volume,
                            &self.unmute_sound_path,
                            crate::sound::SOUND_UNMUTED,
                            &mut self.preview_player,
                        );
                    });
            });

            // ── System section ──
            section_frame(ui, tr("system"), |ui| {
                #[cfg(windows)]
                ui.checkbox(&mut self.autostart, tr("start_windows"));
                #[cfg(not(windows))]
                ui.checkbox(&mut self.autostart, tr("start_system"));
                ui.checkbox(&mut self.notifications_enabled, tr("notifications"));
                ui.checkbox(
                    &mut self.direct_button_enabled,
                    tr("direct_button"),
                )
                .on_hover_text(tr("direct_tip"));
                ui.add_space(4.0);
                egui::Grid::new("system_grid")
                    .num_columns(2)
                    .min_col_width(80.0)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr("log_level"));
                        egui::ComboBox::from_id_salt("log_level_combo")
                            .selected_text(&self.log_level)
                            .show_ui(ui, |ui| {
                                for &level in focusmute_lib::config::VALID_LOG_LEVELS {
                                    ui.selectable_value(
                                        &mut self.log_level,
                                        level.to_string(),
                                        level,
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label(tr("language"));
                        egui::ComboBox::from_id_salt("language_combo")
                            .selected_text(if self.language == "ru" { "Русский" } else { "English" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.language, "en".to_string(), "English");
                                ui.selectable_value(&mut self.language, "ru".to_string(), "Русский");
                            });
                        ui.end_row();
                    });
            });

            }

            // ── Advanced section (collapsible, collapsed by default) ──
            if self.selected_tab == SettingsTab::Advanced {
                section_frame(ui, tr("advanced"), |ui| {
                            let text_width = ui.available_width() - 4.0;
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(tr("webhooks")).strong());
                                ui.label("ℹ").on_hover_ui(|ui| {
                                    ui.label("HTTP POST sent on mute state changes. Body is optional — defaults to {\"event\":\"mute\"} / {\"event\":\"unmute\"}.");
                                });
                            });
                            ui.add_space(2.0);
                            ui.label(tr("on_mute_url"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.on_mute_url)
                                    .desired_width(text_width)
                                    .hint_text("https://example.com/webhook"),
                            );
                            ui.label(tr("body"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.on_mute_body)
                                    .desired_width(text_width)
                                    .hint_text(r#"{"event":"mute"}"#),
                            );
                            ui.add_space(4.0);
                            ui.label(tr("on_unmute_url"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.on_unmute_url)
                                    .desired_width(text_width)
                                    .hint_text("https://example.com/webhook"),
                            );
                            ui.label(tr("body"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.on_unmute_body)
                                    .desired_width(text_width)
                                    .hint_text(r#"{"event":"unmute"}"#),
                            );

                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(tr("browser_sync")).strong());
                                ui.label("ℹ").on_hover_ui(|ui| {
                                    ui.label("Syncs mute state from browser-based meeting apps (Google Meet, Teams) via a browser extension.");
                                    ui.label("Install the FocusMute extension and set the same port here. Default: 9736. Requires restart.");
                                });
                            });
                            ui.add_space(2.0);
                            ui.checkbox(&mut self.suppress_browser_sync_sound, tr("suppress_sound"));
                            ui.add_space(2.0);
                            ui.checkbox(
                                &mut self.browser_sync_reverse,
                                tr("meeting_mute"),
                            )
                            .on_hover_text(
                                "Hotkey and tray mute changes click the meeting's own mute button (Google Meet, Microsoft Teams)",
                            );
                            ui.add_space(2.0);
                            egui::Grid::new("browser_sync_grid")
                                .num_columns(2)
                                .min_col_width(80.0)
                                .spacing([12.0, 8.0])
                                .show(ui, |ui| {
                                    ui.label(tr("port"));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.browser_sync_port)
                                            .desired_width(120.0)
                                            .hint_text("0 = disabled, e.g. 9736"),
                                    );
                                    ui.end_row();
                                });
                });
            }
            // ── About section (collapsible, collapsed by default) ──
            if self.selected_tab == SettingsTab::LedLab {
                section_frame(ui, tr("led_lab_title"), |ui| {
                    ui.add(egui::Label::new(tr("led_lab_intro")).wrap());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("led_snapshot")).clicked() {
                            self.led_lab_snapshot();
                        }
                        if ui.button(tr("led_reset")).clicked() {
                            self.led_lab_reset();
                        }
                    });
                    ui.separator();
                    egui::Grid::new("led_lab_grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(format!("{}: {}", tr("led_index"), led::solo::label(self.led_lab_index)));
                            ui.add(
                                egui::DragValue::new(&mut self.led_lab_index)
                                    .range(0..=31)
                                    .speed(1),
                            );
                            ui.end_row();

                            ui.label("");
                            ui.horizontal(|ui| {
                                if ui.add_enabled(led::solo::can_test(self.led_lab_index), egui::Button::new(tr("led_single_test"))).clicked() {
                                    self.led_lab_single();
                                }
                                if ui.button(tr("led_next_test")).clicked() {
                                    self.led_lab_reset();
                                    self.led_lab_index = match self.led_lab_index { 4 => 6, 6..=18 => self.led_lab_index + 1, _ => 4 };
                                    self.led_lab_single();
                                }
                            });
                            ui.end_row();

                        });
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(tr("led_lab_result")).strong());
                    egui::ScrollArea::vertical()
                        .id_salt("led_lab_result_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.monospace(if self.led_lab_result.is_empty() {
                                "—"
                            } else {
                                &self.led_lab_result
                            });
                        });
                });
            }
            if self.selected_tab == SettingsTab::About {
                section_frame(ui, tr("about"), |ui| {
                            let version = env!("CARGO_PKG_VERSION");
                            ui.label(
                                egui::RichText::new(format!("FocusMute {version}"))
                                    .strong()
                                    .size(15.0),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                tr("about_description"),
                            );
                            ui.add_space(6.0);

                            egui::Grid::new("about_device_grid")
                                .num_columns(2)
                                .spacing([8.0, 4.0])
                                .show(ui, |ui| {
                                    for (key, val) in &self.device_lines {
                                        ui.label(egui::RichText::new(format!("{key}:")).strong());
                                        ui.label(val);
                                        ui.end_row();
                                    }
                                    ui.label("");
                                    ui.end_row();
                                    ui.label(egui::RichText::new(format!("{}:", tr("source"))).strong());
                                    ui.hyperlink_to(
                                        "github.com/SunsetSH/focusmute",
                                        "https://github.com/SunsetSH/focusmute",
                                    );
                                    ui.end_row();
                                });
                });
            }
            // ── Errors area ──
            if !self.errors.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                for err in &self.errors {
                    ui.label(egui::RichText::new(err).color(egui::Color32::from_rgb(220, 50, 50)));
                }
            }
            if let Some(message) = &self.apply_message {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(message).italics());
            }

            // Measure content height BEFORE the button layout. The right-to-left
            // layout below consumes all remaining vertical space, so measuring
            // after it would return the window height (causing a feedback loop).
            content_bottom = ui.cursor().top();

            // ── Buttons ──
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(8.0); // right padding
                // The fill is fixed, so the text color must be too — the
                // theme's light-mode text is near-black and unreadable on
                // the blue fill.
                let save_btn =
                    egui::Button::new(egui::RichText::new(tr("save")).color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(60, 130, 210))
                        .min_size(egui::vec2(80.0, 0.0));
                if ui.add(save_btn).clicked() {
                    self.try_save(ui.ctx());
                }

                if ui.button(tr("apply")).clicked() {
                    self.try_apply();
                }

                // No custom fill: the theme's default button styling stays
                // readable in both light and dark mode.
                let cancel_btn = egui::Button::new(tr("cancel")).min_size(egui::vec2(80.0, 0.0));
                if ui.add(cancel_btn).clicked() {
                    self.cancel(ui.ctx());
                }
            });
        });

        // Clear validation errors when any form field changes.
        if !self.errors.is_empty() && form_snap != self.form_snapshot() {
            self.errors.clear();
        }

        // Always enforce content-driven height (locks vertical resize) while
        // preserving the user's chosen width (horizontal resize is free).
        let target_height = (content_bottom + BUTTON_AREA_HEIGHT).round();
        let current_width = ctx
            .input(|i| i.viewport().inner_rect)
            .map(|r| r.width())
            .unwrap_or(520.0);
        let width = if self.needs_resize {
            520.0
        } else {
            current_width
        };
        self.needs_resize = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            width,
            target_height,
        )));
    }
}

/// Parameters for [`build_and_validate_config`], grouping dialog form fields.
pub(crate) struct ValidateParams<'a> {
    pub color_dirty: &'a ColorDirty,
    pub color_text: &'a str,
    pub color_rgb: [f32; 3],
    pub hotkey: &'a str,
    pub ptt_hotkey: &'a str,
    pub indicator_mode: &'a str,
    pub language: &'a str,
    pub direct_button_enabled: bool,
    pub sound_enabled: bool,
    pub suppress_browser_sync_sound: bool,
    pub mute_sound_volume: f32,
    pub unmute_sound_volume: f32,
    pub autostart: bool,
    pub notifications_enabled: bool,
    pub log_level: &'a str,
    pub mute_inputs_index: usize,
    pub input_count: usize,
    pub mute_sound_path: &'a str,
    pub unmute_sound_path: &'a str,
    pub on_mute_url: &'a str,
    pub on_unmute_url: &'a str,
    pub on_mute_body: &'a str,
    pub on_unmute_body: &'a str,
    pub browser_sync_port: &'a str,
    pub browser_sync_reverse: bool,
    pub blink_on_talk: bool,
    pub blink_while_muted: bool,
    pub talk_threshold: u32,
    pub original: &'a Config,
    pub max_sound_bytes: u64,
}

/// Build a `Config` from dialog form fields, validate, and return it or a list of error strings.
///
/// This is a pure function (no UI side effects) to enable unit testing.
pub(crate) fn build_and_validate_config(p: &ValidateParams<'_>) -> Result<Config, Vec<String>> {
    let mute_inputs = combo_to_mute_inputs(p.mute_inputs_index, p.input_count);

    // Sync color from picker if that was the last change
    let color_str = if *p.color_dirty == ColorDirty::Picker {
        led::rgb_to_hex(p.color_rgb)
    } else {
        p.color_text.to_string()
    };

    // Parse browser_sync_port from string (empty or "0" = disabled)
    let ws_port_str = p.browser_sync_port.trim();
    let browser_sync_port: u16 = if ws_port_str.is_empty() {
        0
    } else {
        match ws_port_str.parse::<u16>() {
            Ok(v) => v,
            Err(_) => {
                return Err(vec![format!(
                    "Invalid browser sync port \"{}\". Enter a number (0 = disabled, e.g. 9736)",
                    p.browser_sync_port
                )]);
            }
        }
    };

    let candidate = Config {
        indicator: focusmute_lib::config::IndicatorConfig {
            mute_color: color_str,
            mute_inputs,
            input_colors: p.original.indicator.input_colors.clone(),
            blink_on_talk: p.blink_on_talk,
            blink_while_muted: p.blink_while_muted,
            talk_threshold: p.talk_threshold,
            mode: p.indicator_mode.to_string(),
        },
        keyboard: focusmute_lib::config::KeyboardConfig {
            hotkey: p.hotkey.to_string(),
            push_to_talk_hotkey: p.ptt_hotkey.trim().to_string(),
        },
        sound: focusmute_lib::config::SoundConfig {
            sound_enabled: p.sound_enabled,
            suppress_browser_sync_sound: p.suppress_browser_sync_sound,
            mute_sound_path: p.mute_sound_path.to_string(),
            unmute_sound_path: p.unmute_sound_path.to_string(),
            mute_sound_volume: p.mute_sound_volume,
            unmute_sound_volume: p.unmute_sound_volume,
        },
        system: focusmute_lib::config::SystemConfig {
            autostart: p.autostart,
            device_serial: p.original.system.device_serial.clone(),
            notifications_enabled: p.notifications_enabled,
            log_level: p.log_level.to_string(),
            browser_sync_port,
            browser_sync_reverse: p.browser_sync_reverse,
            language: p.language.to_string(),
        },
        hooks: focusmute_lib::config::HooksConfig {
            on_mute_url: p.on_mute_url.to_string(),
            on_unmute_url: p.on_unmute_url.to_string(),
            on_mute_body: p.on_mute_body.to_string(),
            on_unmute_body: p.on_unmute_body.to_string(),
        },
        hardware_button: focusmute_lib::config::HardwareButtonConfig {
            direct_button_enabled: p.direct_button_enabled,
            direct_double_click_ms: p.original.hardware_button.direct_double_click_ms,
        },
    };

    let input_count_opt = if p.input_count > 0 {
        Some(p.input_count)
    } else {
        None
    };

    let mut errors = Vec::new();

    if let Err(errs) = candidate.validate(input_count_opt, p.max_sound_bytes) {
        for e in &errs {
            errors.push(e.to_string());
        }
    }

    // Validate hotkey syntax (global-hotkey crate parsing)
    let hotkey_str = p.hotkey.trim();
    let parsed_toggle = hotkey_str.parse::<global_hotkey::hotkey::HotKey>();
    if !hotkey_str.is_empty() && parsed_toggle.is_err() {
        errors.push("Invalid hotkey. Examples: Ctrl+Shift+M, Alt+F1".to_string());
    }

    // Validate PTT hotkey syntax (empty = disabled, which is fine)
    let ptt_str = p.ptt_hotkey.trim();
    if !ptt_str.is_empty() {
        match ptt_str.parse::<global_hotkey::hotkey::HotKey>() {
            Err(_) => {
                errors
                    .push("Invalid push-to-talk hotkey. Examples: Ctrl+Space, Alt+F2".to_string());
            }
            Ok(parsed_ptt) => {
                // Compare parsed hotkey IDs, not strings — catches reordered modifiers
                // like "Ctrl+Shift+M" vs "Shift+Ctrl+M".
                if parsed_toggle
                    .as_ref()
                    .is_ok_and(|t| t.id() == parsed_ptt.id())
                {
                    errors.push(
                        "Push-to-talk hotkey must be different from the toggle hotkey.".to_string(),
                    );
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(candidate)
    } else {
        Err(errors)
    }
}

/// Render a volume slider row inside a sound grid (label + RTL: slider, DragValue, Play).
fn volume_row(
    ui: &mut egui::Ui,
    browse_btn_width: f32,
    control_width: f32,
    language: &str,
    volume: &mut f32,
    sound_path: &str,
    builtin_sound: &'static [u8],
    preview_player: &mut SoundPreviewPlayer,
) {
    ui.label(crate::i18n::tr(language, "volume"));
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let play_btn = egui::Button::new(crate::i18n::tr(language, "play"))
            .min_size(egui::vec2(browse_btn_width, 0.0));
        if ui.add(play_btn).clicked() {
            preview_player.play(sound_path, builtin_sound, *volume);
        }
        let mut pct = *volume * 100.0;
        if ui
            .add(
                egui::DragValue::new(&mut pct)
                    .range(0.0..=100.0)
                    .suffix("%")
                    .max_decimals(0),
            )
            .changed()
        {
            *volume = (pct / 100.0).clamp(0.0, 1.0);
        }
        // The grid gives this row an unconstrained child Ui while measuring.
        // An explicit width prevents the slider from pushing Play off-screen.
        ui.add_sized(
            [
                (control_width - browse_btn_width - 64.0).max(48.0),
                ui.spacing().interact_size.y,
            ],
            egui::Slider::new(volume, 0.0..=1.0).show_value(false),
        );
    });
    ui.end_row();
}

/// Render a section with a title and grouped frame that spans the full width.
fn section_frame(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(title).strong().size(14.0));
    ui.add_space(2.0);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            // Fix both min and max to the frame's available width so all
            // sections render at the same width.
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// Show a native file dialog filtered to WAV files.
fn browse_wav_file() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("WAV", &["wav"])
        .pick_file()
        .and_then(|p| p.to_str().map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn sensitivity_presets_are_valid_and_ordered() {
        // All presets within the 12-bit meter range, and "higher sensitivity"
        // means a strictly lower threshold.
        let values: Vec<u32> = TALK_SENSITIVITY_PRESETS.iter().map(|(_, v)| *v).collect();
        assert!(values.iter().all(|&v| v <= 4095));
        assert!(values.windows(2).all(|w| w[0] > w[1]));
        // The Medium preset is the shipped default.
        assert!(TALK_SENSITIVITY_PRESETS.contains(&("Medium", 250)));
    }

    #[test]
    fn sensitivity_text_names_presets_and_preserves_custom() {
        assert_eq!(sensitivity_text(500), "Low");
        assert_eq!(sensitivity_text(250), "Medium");
        assert_eq!(sensitivity_text(100), "High");
        assert_eq!(sensitivity_text(300), "Custom (300)");
    }

    /// Default valid params — tests override only the fields they care about.
    fn default_test_params(original: &Config) -> ValidateParams<'_> {
        ValidateParams {
            color_dirty: &ColorDirty::Neither,
            color_text: "#FF0000",
            color_rgb: [1.0, 0.0, 0.0],
            hotkey: "Ctrl+Shift+M",
            ptt_hotkey: "",
            indicator_mode: "auto",
            language: "en",
            direct_button_enabled: false,
            browser_sync_reverse: false,
            blink_on_talk: false,
            blink_while_muted: false,
            talk_threshold: 250,
            sound_enabled: true,
            suppress_browser_sync_sound: true,
            mute_sound_volume: 1.0,
            unmute_sound_volume: 1.0,
            autostart: false,
            notifications_enabled: false,
            log_level: "info",
            mute_inputs_index: 0,
            input_count: 2,
            mute_sound_path: "",
            unmute_sound_path: "",
            on_mute_url: "",
            on_unmute_url: "",
            on_mute_body: "",
            on_unmute_body: "",
            browser_sync_port: "0",
            original,
            max_sound_bytes: 10_000_000,
        }
    }

    #[test]
    fn build_valid_inputs_returns_ok() {
        let orig = Config::default();
        let config = build_and_validate_config(&default_test_params(&orig)).expect("should be Ok");
        assert_eq!(config.indicator.mute_color, "#FF0000");
        assert_eq!(config.keyboard.hotkey, "Ctrl+Shift+M");
        assert!(config.sound.sound_enabled);
        assert_eq!(config.sound.mute_sound_volume, 1.0);
        assert_eq!(config.sound.unmute_sound_volume, 1.0);
        assert!(!config.system.autostart);
        assert_eq!(config.indicator.mute_inputs, "all");
    }

    #[test]
    fn build_invalid_color_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Text,
            color_text: "not-a-color",
            color_rgb: [0.0, 0.0, 0.0],
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.to_lowercase().contains("color")),
            "expected color error, got: {errs:?}"
        );
    }

    #[test]
    fn build_empty_hotkey_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            hotkey: "",
            ..default_test_params(&orig)
        });
        // Empty hotkey triggers the Config::validate error (hotkey required)
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.to_lowercase().contains("hotkey")),
            "expected hotkey error, got: {errs:?}"
        );
    }

    #[test]
    fn build_invalid_hotkey_syntax_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            hotkey: "Ctrl+Blah",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("Invalid hotkey")),
            "expected hotkey error, got: {errs:?}"
        );
    }

    #[test]
    fn build_picker_dirty_uses_rgb_conversion() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Picker,
            color_text: "garbage-text",
            color_rgb: [0.0, 1.0, 0.0],
            ..default_test_params(&orig)
        })
        .expect("picker dirty should use RGB, not text");
        assert_eq!(config.indicator.mute_color, "#00FF00");
    }

    #[test]
    fn build_preserves_original_fields() {
        let original = Config {
            indicator: focusmute_lib::config::IndicatorConfig {
                input_colors: HashMap::from([("1".into(), "#00FF00".into())]),
                ..Default::default()
            },
            system: focusmute_lib::config::SystemConfig {
                device_serial: "ABC123".to_string(),
                ..Default::default()
            },
            ..Config::default()
        };

        let config = build_and_validate_config(&ValidateParams {
            notifications_enabled: true,
            ..default_test_params(&original)
        })
        .expect("should be Ok");

        assert_eq!(config.system.device_serial, "ABC123");
        assert_eq!(config.indicator.input_colors.get("1").unwrap(), "#00FF00");
        // notifications_enabled comes from the form param, not original
        assert!(config.system.notifications_enabled);
    }

    #[test]
    fn build_hooks_are_preserved() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            on_mute_url: "https://example.com/mute",
            on_unmute_url: "https://example.com/unmute",
            on_mute_body: r#"{"muted":true}"#,
            on_unmute_body: r#"{"muted":false}"#,
            ..default_test_params(&orig)
        })
        .expect("should be Ok");

        assert_eq!(config.hooks.on_mute_url, "https://example.com/mute");
        assert_eq!(config.hooks.on_unmute_url, "https://example.com/unmute");
        assert_eq!(config.hooks.on_mute_body, r#"{"muted":true}"#);
        assert_eq!(config.hooks.on_unmute_body, r#"{"muted":false}"#);
    }

    // NOTE: Color conversion tests (hex_to_rgb, rgb_to_hex, roundtrips) removed —
    // fully covered by led::color::tests in focusmute-lib.

    // ── T2: Additional settings dialog validation tests ──

    #[test]
    fn build_multiple_simultaneous_errors() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Text,
            color_text: "not-a-color",
            color_rgb: [0.0, 0.0, 0.0],
            hotkey: "",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.len() >= 2,
            "should collect multiple errors, got {}: {errs:?}",
            errs.len()
        );
        assert!(errs.iter().any(|e| e.to_lowercase().contains("color")));
        assert!(errs.iter().any(|e| e.to_lowercase().contains("hotkey")));
    }

    #[test]
    fn build_whitespace_only_color_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Text,
            color_text: "   ",
            color_rgb: [0.0, 0.0, 0.0],
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.to_lowercase().contains("color")),
            "expected color error, got: {errs:?}"
        );
    }

    #[test]
    fn build_picker_dirty_overrides_invalid_text() {
        // When picker is dirty, the RGB value is used even if color_text is invalid.
        // This tests that validation passes because the picker value is valid.
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Picker,
            color_text: "invalid",
            color_rgb: [0.5, 0.5, 0.5],
            ..default_test_params(&orig)
        });
        assert!(
            result.is_ok(),
            "picker dirty should use RGB, ignoring invalid text"
        );
        let config = result.unwrap();
        assert_eq!(config.indicator.mute_color, "#808080");
    }

    #[test]
    fn build_independent_sound_volumes() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            mute_sound_volume: 0.3,
            unmute_sound_volume: 0.8,
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert_eq!(config.sound.mute_sound_volume, 0.3);
        assert_eq!(config.sound.unmute_sound_volume, 0.8);
    }

    #[test]
    fn build_sound_volume_out_of_range_returns_err() {
        let orig = Config::default();
        for bad in [1.5, -0.1] {
            let result = build_and_validate_config(&ValidateParams {
                mute_sound_volume: bad,
                ..default_test_params(&orig)
            });
            assert!(
                result.is_err(),
                "mute_sound_volume {bad} should fail validation"
            );
            let errs = result.unwrap_err();
            assert!(
                errs.iter().any(|e| e.to_lowercase().contains("volume")),
                "expected volume error for {bad}, got: {errs:?}"
            );
        }
    }

    #[test]
    fn build_notifications_enabled_true_preserved() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            notifications_enabled: true,
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert!(config.system.notifications_enabled);
    }

    #[test]
    fn build_nan_sound_volume_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            mute_sound_volume: f32::NAN,
            ..default_test_params(&orig)
        });
        assert!(
            result.is_err(),
            "NaN mute_sound_volume should fail validation"
        );
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.to_lowercase().contains("volume")),
            "expected volume error for NaN, got: {errs:?}"
        );
    }

    #[test]
    fn build_text_dirty_uses_text_not_rgb() {
        // When color_dirty is Text and text is valid, the text value should be used
        // (not the RGB picker value).
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            color_dirty: &ColorDirty::Text,
            color_text: "#00FF00",
            color_rgb: [1.0, 0.0, 0.0], // red — should be ignored
            ..default_test_params(&orig)
        })
        .expect("valid text color should succeed");
        assert_eq!(config.indicator.mute_color, "#00FF00");
    }

    // ── v0.7.4: user-friendly error messages ──

    #[test]
    fn build_invalid_hotkey_shows_examples() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            hotkey: "Not+A+Real+Key",
            ..default_test_params(&orig)
        });
        let errs = result.unwrap_err();
        let hotkey_err = errs.iter().find(|e| e.contains("hotkey")).unwrap();
        assert!(
            hotkey_err.contains("Examples"),
            "should show examples, got: {hotkey_err}"
        );
        assert!(
            hotkey_err.contains("Ctrl+Shift+M"),
            "should include Ctrl+Shift+M example, got: {hotkey_err}"
        );
    }

    #[test]
    fn build_valid_hotkey_no_error() {
        for hk in &["Ctrl+Shift+M", "Alt+F1", "F12", "Ctrl+M"] {
            let orig = Config::default();
            let result = build_and_validate_config(&ValidateParams {
                hotkey: hk,
                ..default_test_params(&orig)
            });
            assert!(result.is_ok(), "hotkey '{hk}' should be valid");
        }
    }

    #[test]
    fn build_ptt_hotkey_preserved_in_config() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            ptt_hotkey: "Ctrl+Space",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert_eq!(config.keyboard.push_to_talk_hotkey, "Ctrl+Space");
    }

    #[test]
    fn build_empty_ptt_hotkey_means_disabled() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            ptt_hotkey: "",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert!(config.keyboard.push_to_talk_hotkey.is_empty());
    }

    #[test]
    fn build_invalid_ptt_hotkey_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            ptt_hotkey: "Not+A+Real+Key",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("push-to-talk")),
            "expected PTT error, got: {errs:?}"
        );
    }

    #[test]
    fn build_ptt_same_as_toggle_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            hotkey: "Ctrl+Shift+M",
            ptt_hotkey: "Ctrl+Shift+M",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("different")),
            "expected duplicate hotkey error, got: {errs:?}"
        );
    }

    #[test]
    fn build_ptt_same_as_toggle_reordered_modifiers_returns_err() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            hotkey: "Ctrl+Shift+M",
            ptt_hotkey: "Shift+Ctrl+M",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("different")),
            "reordered modifiers should still detect duplicate, got: {errs:?}"
        );
    }

    #[test]
    fn build_ptt_whitespace_treated_as_disabled() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            ptt_hotkey: "   ",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert!(config.keyboard.push_to_talk_hotkey.is_empty());
    }

    // ── WebSocket port ──

    #[test]
    fn build_valid_browser_sync_port() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            browser_sync_port: "9736",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert_eq!(config.system.browser_sync_port, 9736);
    }

    #[test]
    fn build_browser_sync_port_zero_is_disabled() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            browser_sync_port: "0",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert_eq!(config.system.browser_sync_port, 0);
    }

    #[test]
    fn build_browser_sync_port_empty_is_disabled() {
        let orig = Config::default();
        let config = build_and_validate_config(&ValidateParams {
            browser_sync_port: "",
            ..default_test_params(&orig)
        })
        .expect("should be Ok");
        assert_eq!(config.system.browser_sync_port, 0);
    }

    #[test]
    fn build_browser_sync_port_invalid_string() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            browser_sync_port: "abc",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("browser sync port")),
            "expected port parse error, got: {errs:?}"
        );
    }

    #[test]
    fn build_browser_sync_port_privileged_rejected() {
        let orig = Config::default();
        let result = build_and_validate_config(&ValidateParams {
            browser_sync_port: "80",
            ..default_test_params(&orig)
        });
        assert!(result.is_err());
        let errs = result.unwrap_err();
        assert!(
            errs.iter().any(|e| e.to_lowercase().contains("privileged")),
            "expected privileged port error, got: {errs:?}"
        );
    }
}
