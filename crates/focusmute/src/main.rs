//! FocusMute — hotkey mute control for Focusrite Scarlett 4th Gen interfaces.
//!
//! GUI subsystem: double-click from Explorer launches the system tray.
//! If run from a terminal with arguments, redirects the user to focusmute-cli.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(windows, target_os = "linux"))]
mod i18n;
#[cfg(any(windows, target_os = "linux"))]
mod icon;
#[cfg(any(windows, target_os = "linux"))]
mod notification;
#[cfg(any(windows, target_os = "linux"))]
mod osd;
#[cfg(any(windows, target_os = "linux"))]
mod settings_dialog;
#[cfg(any(windows, target_os = "linux"))]
mod sound;
#[cfg(any(windows, target_os = "linux"))]
mod tray;

#[cfg(any(windows, target_os = "linux"))]
use std::sync::OnceLock;
#[cfg(any(windows, target_os = "linux"))]
use std::sync::atomic::AtomicBool;

/// Shared shutdown flag — set by tray quit.
#[cfg(any(windows, target_os = "linux"))]
pub static RUNNING: AtomicBool = AtomicBool::new(true);

/// Logger handle for runtime log level changes from the settings dialog.
#[cfg(any(windows, target_os = "linux"))]
static LOGGER_HANDLE: OnceLock<flexi_logger::LoggerHandle> = OnceLock::new();

/// Change the log level at runtime (called from the settings dialog).
#[cfg(any(windows, target_os = "linux"))]
pub fn set_log_level(level: &str) {
    if let Some(handle) = LOGGER_HANDLE.get()
        && let Err(e) = handle.parse_new_spec(level)
    {
        log::warn!("[config] could not set log level to '{level}': {e}");
    }
}

/// Check if we were launched from an interactive console (PowerShell, cmd, etc.).
#[cfg(windows)]
fn has_parent_console() -> bool {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole};

    // SAFETY: AttachConsole/FreeConsole are standard Win32 console APIs.
    // AttachConsole probes for a parent console; FreeConsole detaches it.
    // Both are safe to call from any thread before any console I/O.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            // We successfully attached — there's a parent console.
            // Detach immediately since this is the tray binary.
            let _ = FreeConsole();
            true
        } else {
            false
        }
    }
}

/// Initialize the tray app logger, directing output to a log file.
///
/// Falls back to stderr if the log file can't be opened.
/// Reads the configured log level from config; defaults to "info".
/// Stores the logger handle in [`LOGGER_HANDLE`] for runtime level changes.
fn init_tray_logger() {
    use flexi_logger::{FileSpec, Logger, WriteMode};
    use focusmute_lib::config::Config;

    let config = Config::load();
    let level = if focusmute_lib::config::VALID_LOG_LEVELS
        .contains(&config.system.log_level.to_lowercase().as_str())
    {
        config.system.log_level.as_str()
    } else {
        "info"
    };

    let logger =
        Logger::try_with_str(level).unwrap_or_else(|_| Logger::try_with_str("info").unwrap());

    let logger = if let Some(log_path) = Config::log_path() {
        if let Some(dir) = log_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        roll_over_log(&log_path, MAX_LOG_BYTES);
        // FileSpec from the full path: directory + basename + suffix.
        let file_spec = FileSpec::try_from(log_path).unwrap_or_default();
        // Appending keeps the previous session: a problem the user reports
        // after restarting must still be in the log.
        logger
            .log_to_file(file_spec)
            .append()
            .write_mode(WriteMode::Direct)
    } else {
        logger
    };

    match logger.format(focusmute_log_format).start() {
        Ok(handle) => {
            let _ = LOGGER_HANDLE.set(handle);
        }
        Err(e) => eprintln!("Failed to initialize logger: {e}"),
    }
}

/// Size at which the log is moved aside on start.
const MAX_LOG_BYTES: u64 = 1_000_000;

/// Move `path` to `<name>.old.<ext>` once it has grown past `max_bytes`, so
/// the appended log keeps one previous generation instead of growing forever.
/// The rotation happens only at start: the file name stays the one the
/// documentation names.
fn roll_over_log(path: &std::path::Path, max_bytes: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= max_bytes {
        return;
    }
    let old = path.with_extension("old.log");
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(path, &old) {
        eprintln!("Could not rotate {}: {e}", path.display());
    }
}

/// Log format: `[timestamp] LEVEL message`
fn focusmute_log_format(
    w: &mut dyn std::io::Write,
    now: &mut flexi_logger::DeferredNow,
    record: &log::Record,
) -> std::io::Result<()> {
    write!(
        w,
        "[{}] {:5} {}",
        now.format(flexi_logger::TS_DASHES_BLANK_COLONS_DOT_BLANK),
        record.level(),
        record.args(),
    )
}

fn main() {
    init_tray_logger();

    #[cfg(not(any(windows, target_os = "linux")))]
    {
        eprintln!("The tray app is only available on Windows and Linux.");
        eprintln!("Use focusmute-cli for command-line usage.");
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();

        // If launched with CLI arguments from a terminal, redirect to focusmute-cli
        if args.len() > 1 && has_parent_console() {
            eprintln!("Hint: Use focusmute-cli.exe for command-line usage.");
            eprintln!("  Example: focusmute-cli.exe {}", args[1..].join(" "));
            return;
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    {
        if let Err(e) = tray::run() {
            let msg = format!("Error: {e}");
            eprintln!("{msg}");
            show_fatal_error(&msg);
            std::process::exit(1);
        }
    }
}

/// Show a fatal error to the user. On Windows, displays a MessageBox since the
/// tray binary has no console. On other platforms, the eprintln above suffices.
#[cfg(windows)]
fn show_fatal_error(msg: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;

    let wide_msg: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "FocusMute"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(wide_msg.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_ICONERROR | MB_OK,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal_error(_msg: &str) {
    // On non-Windows platforms, eprintln above is visible from the terminal.
}

#[cfg(test)]
mod tests {
    use super::roll_over_log;

    #[test]
    fn log_is_moved_aside_only_past_the_limit() {
        let dir = std::env::temp_dir().join("focusmute_test_log_rollover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("focusmute.log");
        let old = dir.join("focusmute.old.log");

        std::fs::write(&log, b"small").unwrap();
        roll_over_log(&log, 10);
        assert!(log.exists() && !old.exists(), "a small log is kept");

        std::fs::write(&old, b"older").unwrap();
        std::fs::write(&log, b"past the limit").unwrap();
        roll_over_log(&log, 10);
        assert!(!log.exists());
        assert_eq!(std::fs::read(&old).unwrap(), b"past the limit");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
