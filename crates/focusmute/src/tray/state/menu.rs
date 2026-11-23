//! Tray menu construction, notifications, and mute-state UI updates.

use std::cell::RefCell;

use focusmute_lib::config::Config;
use focusmute_lib::monitor::MonitorAction;

use muda::{Menu, MenuItem, PredefinedMenuItem};

use super::icon::{icon_disconnected, icon_live, icon_muted};
use super::{TrayResources, TrayState};
use crate::i18n::tr;
use crate::sound;

// ── What the tray shows ──

/// The microphone side of the tray.
#[derive(Clone, Debug, PartialEq)]
pub enum MicStatus {
    /// Windows reports no capture device yet.
    Missing,
    /// The microphone mute applies to. `stale` while Windows has moved the
    /// default microphone and FocusMute could not open the new one yet.
    Watching { name: Option<String>, stale: bool },
}

/// Everything the tray icon, tooltip and information lines show. Kept in one
/// place so no update path can leave one of them out of date.
#[derive(Clone, Debug, PartialEq)]
pub struct TrayView {
    pub muted: bool,
    pub mic: MicStatus,
    /// The Scarlett's name while it is connected.
    pub device: Option<String>,
}

/// Windows cuts tray tooltips at 127 characters.
const TOOLTIP_MAX: usize = 127;

/// Tooltip: the state first, then what qualifies it, the microphone name
/// last, because it is the part that may be cut.
fn tooltip(language: &str, view: &TrayView) -> String {
    let state = match view.mic {
        MicStatus::Missing => tr(language, "no_mic"),
        _ if view.muted => tr(language, "muted"),
        _ => tr(language, "live"),
    };
    let mut text = format!("FocusMute — {state}");
    if matches!(view.mic, MicStatus::Watching { stale: true, .. }) {
        text.push_str(" · ");
        text.push_str(tr(language, "mic_switching"));
    }
    if view.device.is_none() {
        text.push_str(" · ");
        text.push_str(tr(language, "scarlett_disconnected"));
    }
    if let MicStatus::Watching {
        name: Some(name), ..
    } = &view.mic
        && !name.is_empty()
    {
        text.push_str(" · ");
        text.push_str(name);
    }
    text.chars().take(TOOLTIP_MAX).collect()
}

/// Menu line naming the microphone mute applies to.
fn mic_line(language: &str, mic: &MicStatus) -> String {
    match mic {
        MicStatus::Missing => tr(language, "mic_missing_hint").to_string(),
        MicStatus::Watching { name, stale } => {
            let name = name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or(tr(language, "mic_unnamed"));
            let mut line = format!("{}: {name}", tr(language, "mic_label"));
            if *stale {
                line.push_str(&format!(" ({})", tr(language, "mic_switching")));
            }
            line
        }
    }
}

/// Model name without the serial suffix the driver appends
/// ("Scarlett Solo 4th Gen-000171cf" → "Scarlett Solo 4th Gen").
fn model_name(device_name: &str) -> &str {
    match device_name.rsplit_once('-') {
        Some((model, suffix))
            if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            model
        }
        _ => device_name,
    }
}

// ── Shared menu construction ──

/// All menu items the tray uses, returned from `build_tray_menu`.
pub struct TrayMenu {
    /// Mute state of the microphone, or that there is none.
    pub status_item: MenuItem,
    /// Which microphone mute applies to.
    pub mic_item: MenuItem,
    /// The connected Scarlett, or a prompt to connect one.
    pub device_item: MenuItem,
    pub toggle_item: MenuItem,
    pub settings_item: MenuItem,
    pub reconnect_item: MenuItem,
    pub quit_item: MenuItem,
    language: String,
    /// The tray lives on the main thread only.
    view: RefCell<TrayView>,
    /// The icon the menu belongs to; set by [`build_tray_icon`].
    tray: Option<tray_icon::TrayIcon>,
}

impl TrayMenu {
    /// Update the reconnect menu item label based on device connection status.
    fn set_reconnect_label(&self, connected: bool) {
        self.reconnect_item.set_text(tr(
            &self.language,
            if connected { "resync" } else { "reconnect" },
        ));
    }

    /// Paint the icon, tooltip and information lines from the view.
    fn render(&self) {
        let Some(tray) = &self.tray else {
            return;
        };
        let view = self.view.borrow();
        let icon = match (&view.device, view.muted) {
            (None, _) => icon_disconnected(),
            (Some(_), true) => icon_muted(),
            (Some(_), false) => icon_live(),
        };
        tray.set_icon(Some(icon)).ok();
        tray.set_tooltip(Some(tooltip(&self.language, &view))).ok();
        self.status_item.set_text(match view.mic {
            MicStatus::Missing => tr(&self.language, "no_mic"),
            _ if view.muted => tr(&self.language, "muted"),
            _ => tr(&self.language, "live"),
        });
        self.mic_item.set_text(mic_line(&self.language, &view.mic));
        match &view.device {
            Some(name) => self.device_item.set_text(model_name(name)),
            None => self
                .device_item
                .set_text(tr(&self.language, "connect_device")),
        }
        self.set_reconnect_label(view.device.is_some());
    }

    /// Change the view and repaint if anything differs.
    fn update(&self, change: impl FnOnce(&mut TrayView)) {
        let mut next = self.view.borrow().clone();
        change(&mut next);
        if next != *self.view.borrow() {
            *self.view.borrow_mut() = next;
            self.render();
        }
    }

    /// Show a mute state. It belongs to the microphone, so it is shown with
    /// the Scarlett unplugged too: the icon then keeps showing the missing
    /// device, the tooltip and the status line name the state.
    pub fn set_muted(&self, muted: bool) {
        self.update(|v| v.muted = muted);
    }

    pub fn set_mic(&self, mic: MicStatus) {
        self.update(|v| v.mic = mic);
    }

    /// Show the connected Scarlett by its name, or that it is missing.
    pub fn set_device(&self, device_name: Option<&str>) {
        self.update(|v| v.device = device_name.map(str::to_owned));
    }

    /// Repaint all persistent tray labels after a language or hotkey change.
    pub fn sync_language(&mut self, config: &Config) {
        self.language = config.system.language.clone();
        self.toggle_item.set_text(format!(
            "{}\t{}",
            tr(&self.language, "toggle_mute"),
            config.keyboard.hotkey
        ));
        self.settings_item
            .set_text(format!("{}...", tr(&self.language, "settings")));
        self.quit_item.set_text(tr(&self.language, "quit"));
        self.render();
    }
}

/// Build the tray context menu with all standard items. Its labels are
/// painted from `view` once the icon exists ([`build_tray_icon`]).
pub fn build_tray_menu(config: &Config, view: TrayView) -> (Menu, TrayMenu) {
    let menu = Menu::new();
    let language = &config.system.language;
    let status_item = MenuItem::new("", false, None);
    let mic_item = MenuItem::new("", false, None);
    let device_item = MenuItem::new("", false, None);
    let toggle_label = format!(
        "{}\t{}",
        tr(language, "toggle_mute"),
        config.keyboard.hotkey
    );
    let toggle_item = MenuItem::new(&toggle_label, true, None);
    let settings_item = MenuItem::new(format!("{}...", tr(language, "settings")), true, None);
    let reconnect_item = MenuItem::new(tr(language, "reconnect"), true, None);
    let quit_item = MenuItem::new(tr(language, "quit"), true, None);

    let append = |item: &dyn muda::IsMenuItem| {
        if let Err(e) = menu.append(item) {
            log::warn!("[menu] could not append menu item: {e}");
        }
    };
    append(&status_item);
    append(&mic_item);
    append(&device_item);
    append(&PredefinedMenuItem::separator());
    append(&toggle_item);
    append(&PredefinedMenuItem::separator());
    append(&settings_item);
    append(&reconnect_item);
    append(&PredefinedMenuItem::separator());
    append(&quit_item);

    (
        menu,
        TrayMenu {
            status_item,
            mic_item,
            device_item,
            toggle_item,
            settings_item,
            reconnect_item,
            quit_item,
            language: language.clone(),
            view: RefCell::new(view),
            tray: None,
        },
    )
}

/// Build the tray icon, hand it to `tray_menu` and paint it, with the menu
/// labels, from the view.
pub fn build_tray_icon(menu: Menu, tray_menu: &mut TrayMenu) -> focusmute_lib::error::Result<()> {
    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("FocusMute")
        .with_icon(icon_disconnected())
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| {
            focusmute_lib::FocusmuteError::Config(format!("Failed to create tray icon: {e}"))
        })?;
    tray_menu.tray = Some(tray);
    tray_menu.render();
    Ok(())
}

/// Show startup warnings as a desktop notification.
///
/// Always shown regardless of `notifications_enabled` — if the config is broken,
/// that flag itself may be wrong.
pub(crate) fn show_startup_warnings(language: &str, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    let body = warnings.join("\n");
    crate::notification::Notifier::show_oneshot(&format!(
        "{}\n{body}",
        tr(language, "notify_config_warnings")
    ));
}

/// Apply mute-state UI updates to the tray icon and status item.
///
/// The mute belongs to the Windows microphone, so sound, notification and
/// hooks follow it even while the Scarlett is unplugged.
pub fn apply_mute_ui(
    action: MonitorAction,
    menu: &TrayMenu,
    state: &TrayState,
    resources: &mut TrayResources,
    suppress_sound: bool,
) {
    let play_sound = state.config.sound.sound_enabled && !suppress_sound;
    let language = &state.config.system.language;
    match action {
        MonitorAction::ApplyMute => {
            log::info!("[mute] muted");
            menu.set_muted(true);
            if play_sound {
                sound::play_sound(&resources.mute_sound, state.config.sound.mute_sound_volume);
            }
            if state.config.system.notifications_enabled {
                resources
                    .notifier
                    .show_mute_state(tr(language, "notify_muted"));
            }
        }
        MonitorAction::ClearMute => {
            log::info!("[mute] unmuted");
            menu.set_muted(false);
            if play_sound {
                sound::play_sound(
                    &resources.unmute_sound,
                    state.config.sound.unmute_sound_volume,
                );
            }
            if state.config.system.notifications_enabled {
                resources
                    .notifier
                    .show_mute_state(tr(language, "notify_live"));
            }
        }
        MonitorAction::NoChange => {}
    }
    focusmute_lib::hooks::run_action_hook(action, &state.config);
}

#[cfg(test)]
mod tests {
    use super::{MicStatus, TrayView, mic_line, model_name, tooltip};

    fn view(muted: bool, mic: Option<&str>, device: bool) -> TrayView {
        TrayView {
            muted,
            mic: MicStatus::Watching {
                name: mic.map(str::to_owned),
                stale: false,
            },
            device: device.then(|| "Scarlett Solo 4th Gen-000171cf".to_owned()),
        }
    }

    #[test]
    fn tooltip_names_state_microphone_and_missing_scarlett() {
        assert_eq!(tooltip("en", &view(true, None, true)), "FocusMute — Muted");
        assert_eq!(
            tooltip("en", &view(false, Some("Focusrite USB Audio"), true)),
            "FocusMute — Live · Focusrite USB Audio"
        );
        assert_eq!(
            tooltip("en", &view(true, Some("Webcam"), false)),
            "FocusMute — Muted · Scarlett disconnected · Webcam"
        );
    }

    #[test]
    fn tooltip_is_translated() {
        assert_eq!(
            tooltip("ru", &view(true, Some("Webcam"), false)),
            "FocusMute — Микрофон выключен · Scarlett отключена · Webcam"
        );
    }

    #[test]
    fn tooltip_shows_a_missing_and_a_stale_microphone() {
        let mut v = view(false, None, true);
        v.mic = MicStatus::Missing;
        assert_eq!(tooltip("en", &v), "FocusMute — No microphone");
        v.mic = MicStatus::Watching {
            name: Some("Webcam".into()),
            stale: true,
        };
        assert_eq!(
            tooltip("en", &v),
            "FocusMute — Live · waiting for the new microphone · Webcam"
        );
    }

    /// Windows cuts tooltips at 127 characters; the state must survive.
    #[test]
    fn tooltip_stays_within_the_windows_limit() {
        let long = "x".repeat(300);
        for language in ["en", "ru"] {
            let text = tooltip(language, &view(true, Some(&long), false));
            assert!(text.chars().count() <= 127, "{}", text.chars().count());
            assert!(text.contains("Scarlett"), "{text}");
        }
    }

    #[test]
    fn menu_line_names_the_microphone() {
        assert_eq!(
            mic_line(
                "en",
                &MicStatus::Watching {
                    name: Some("Webcam".into()),
                    stale: false
                }
            ),
            "Microphone: Webcam"
        );
        assert_eq!(
            mic_line(
                "ru",
                &MicStatus::Watching {
                    name: None,
                    stale: true
                }
            ),
            "Микрофон: без имени (ожидание нового микрофона)"
        );
        assert_eq!(
            mic_line("en", &MicStatus::Missing),
            "Windows reports no recording device"
        );
    }

    #[test]
    fn model_name_drops_only_a_serial_suffix() {
        assert_eq!(
            model_name("Scarlett Solo 4th Gen-000171cf"),
            "Scarlett Solo 4th Gen"
        );
        assert_eq!(model_name("Scarlett 2i2 4th Gen"), "Scarlett 2i2 4th Gen");
        assert_eq!(model_name("Some-Name"), "Some-Name");
    }
}
