//! Wakes the tray's main loop from background threads.
//!
//! The Windows loop blocks in `MsgWaitForMultipleObjects`, which returns on
//! window messages but not on channel sends. A thread message posted after
//! each send makes the loop handle it at once instead of at its next timeout.
//! On Linux the GTK loop keeps its 50 ms timer, so waking is a no-op there.
//!
//! While the settings window is open its event loop runs the tray instead;
//! it registers its egui context here so a wake repaints it, which runs the
//! tray step from the window's `logic`.

use std::sync::Mutex;
#[cfg(windows)]
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(windows)]
static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// The open settings window, if any.
static SETTINGS_CTX: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);

/// Record the calling thread as the one running the main loop.
pub fn register_main_thread() {
    #[cfg(windows)]
    MAIN_THREAD_ID.store(
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
        Ordering::SeqCst,
    );
}

/// Route wakes to the settings window while it is open; `None` on close.
pub fn set_settings_context(ctx: Option<eframe::egui::Context>) {
    if let Ok(mut slot) = SETTINGS_CTX.lock() {
        *slot = ctx;
    }
}

/// Make the main loop's wait return now.
pub fn wake_main() {
    if let Ok(slot) = SETTINGS_CTX.lock()
        && let Some(ctx) = slot.as_ref()
    {
        ctx.request_repaint();
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP};

        let id = MAIN_THREAD_ID.load(Ordering::SeqCst);
        if id != 0 {
            // A lost wake (e.g. during a modal menu loop) only delays handling
            // until the next timeout, so the result is deliberately ignored.
            let _ = unsafe { PostThreadMessageW(id, WM_APP, WPARAM(0), LPARAM(0)) };
        }
    }
}
