//! LED control — single-LED update, mute indicator apply/clear/restore.

mod color;
mod ops;
pub mod solo;
mod strategy;

pub use color::{color_to_rgb, format_color, parse_color, rgb_to_hex};
pub use ops::{
    apply_mute_indicator, apply_mute_indicator_with_mode, blank_mute_indicator,
    clear_mute_indicator, clear_mute_indicator_with_mode, refresh_after_reconnect, restore_on_exit,
    set_single_led,
};
pub use strategy::{
    IndicatorRenderMode, MuteStrategy, mute_color_or_default, resolve_strategy_from_config,
};
