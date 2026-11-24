//! Shared tray event loop — extracted from the ~80% identical code in
//! `windows.rs` and `linux.rs`. Platform-specific behavior is injected
//! via the [`PlatformAdapter`] trait.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use focusmute_lib::audio::MuteMonitor;
use focusmute_lib::config::Config;
use focusmute_lib::device::{PlatformDevice, ScarlettDevice, open_device_by_serial};
use focusmute_lib::models::PanelControls;
use focusmute_lib::monitor::MonitorAction;

use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use muda::MenuEvent;

use super::state::{self, Msg, TrayResources, TrayState};
use crate::RUNNING;

/// Platform-specific hooks that differ between Windows and Linux.
///
/// Each platform implements this trait once; `run_core` provides the
/// shared event loop, config load, device open, menu build, etc.
pub trait PlatformAdapter {
    type Monitor: MuteMonitor + Send + Sync + 'static;

    /// One-time platform init (GTK, COM, etc.). Called before anything else.
    fn platform_init() -> focusmute_lib::error::Result<()>;

    /// Create the audio mute monitor on the **main thread**, on the
    /// Scarlett's input with `prefer_scarlett` where the platform can choose.
    /// Returns `None` if the audio subsystem is unavailable.
    fn create_monitor(prefer_scarlett: bool) -> Option<Self::Monitor>;

    /// Spawn the background polling thread.
    /// The thread should call `monitor.wait_for_change()` / `refresh()` / `is_muted()`
    /// and send `Msg::MutePoll` over `tx`.
    fn spawn_poll_thread(monitor: Arc<Self::Monitor>, tx: mpsc::Sender<Msg>) -> JoinHandle<()>;

    /// Pump platform-specific events (Win32 messages, GTK iterations).
    fn pump_events();

    /// Pump events the settings window's own event loop does not see, from
    /// inside it. Windows needs nothing: winit dispatches every message of
    /// the thread, the tray's and the hotkeys' windows included.
    fn pump_events_nested() {}

    /// Block until the next platform event, a [`super::wake::wake_main`]
    /// call, or `timeout`. Linux keeps its fixed 50 ms GTK timer instead.
    fn wait_for_events(timeout: Duration);

    /// Register for device hotplug notifications (called once at startup).
    fn register_device_notifications() {}

    /// Check whether a device-removed event has fired since the last call.
    /// Resets the flag so subsequent calls return false until the next event.
    fn check_device_removed() -> bool {
        false
    }

    /// Check whether a device-arrival event has fired since the last call.
    fn check_device_arrived() -> bool {
        false
    }
}

/// Idle period of the mute poll thread: the safety net for a missed
/// audio-system callback.
const POLL_IDLE: Duration = Duration::from_millis(250);

/// Delay before the confirming sample after a mute event. The debouncer needs
/// two equal samples; taking the second one right away instead of after
/// [`POLL_IDLE`] cuts the hotkey-to-LED latency from ~280 ms to ~30 ms while
/// still filtering one-sample COM/endpoint flicker.
const POLL_CONFIRM: Duration = Duration::from_millis(25);

/// Main-loop wait while something is timed: a blink, a Direct gesture, a
/// muted-talk blink, or a pending reconnect.
const LOOP_BUSY: Duration = Duration::from_millis(50);

/// Main-loop wait when idle. Every input that matters (tray, hotkeys,
/// background messages) wakes the loop early, so this is only a fallback.
const LOOP_IDLE: Duration = Duration::from_millis(500);

/// One mute sample. The generation is read first: if it is already the
/// current one, the state read after it belongs to that microphone.
fn mute_sample<M: MuteMonitor>(monitor: &M) -> Msg {
    let generation = monitor.generation();
    monitor.refresh();
    Msg::MutePoll {
        muted: monitor.is_muted(),
        generation,
    }
}

/// Body of the platform mute poll threads.
pub fn run_poll_loop<M: MuteMonitor>(monitor: &M, tx: &mpsc::Sender<Msg>) {
    while crate::RUNNING.load(Ordering::SeqCst) {
        let woke = monitor.wait_for_change(POLL_IDLE);
        if tx.send(mute_sample(monitor)).is_err() {
            break;
        }
        if !woke {
            // Idle samples need no prompt handling; the loop drains them at
            // its next wake.
            continue;
        }
        super::wake::wake_main();
        std::thread::sleep(POLL_CONFIRM);
        if tx.send(mute_sample(monitor)).is_err() {
            break;
        }
        super::wake::wake_main();
    }
}

/// Background listener for Direct-button notifications.
///
/// Exists only while the Direct option is on and the connected device has
/// panel controls (a Solo), so a disabled option costs no thread and no
/// device I/O.
struct DirectWatcher {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl DirectWatcher {
    /// How long one notification wait blocks before re-checking `stop`.
    const WAIT_MS: u32 = 1000;
    /// Minimum gap between restarts after the listener thread exits.
    const RETRY: Duration = Duration::from_secs(2);

    fn spawn(
        listener: Box<dyn focusmute_lib::device::NotifyListener>,
        tx: mpsc::Sender<Msg>,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("direct-notify".into())
            .spawn(move || {
                log::debug!("[direct] listener started");
                while !thread_stop.load(Ordering::SeqCst) && crate::RUNNING.load(Ordering::SeqCst) {
                    match listener.wait(Self::WAIT_MS) {
                        Ok(Some(packet)) => {
                            if tx.send(Msg::DirectNotify(packet)).is_err() {
                                break;
                            }
                            super::wake::wake_main();
                        }
                        Ok(None) => {}
                        Err(e) => {
                            // Device gone or channel unusable; the main loop
                            // restarts the listener if Direct is still wanted.
                            log::debug!("[direct] listener stopped: {e}");
                            break;
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { stop, handle })
    }

    /// Ask the thread to exit; it does so within [`Self::WAIT_MS`].
    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Start, stop or restart the Direct listener to match `wanted`.
fn sync_direct_watcher(
    watcher: &mut Option<DirectWatcher>,
    last_spawn: &mut Option<Instant>,
    wanted: bool,
    device: Option<&impl ScarlettDevice>,
    tx: &mpsc::Sender<Msg>,
) {
    if watcher.as_ref().is_some_and(|w| w.handle.is_finished()) {
        *watcher = None;
    }
    if !wanted {
        if let Some(w) = watcher.take() {
            w.stop();
            log::debug!("[direct] listener stopping");
        }
        return;
    }
    if watcher.is_some() || last_spawn.is_some_and(|at| at.elapsed() < DirectWatcher::RETRY) {
        return;
    }
    let Some(listener) = device.and_then(|d| d.notify_listener()) else {
        return;
    };
    *last_spawn = Some(Instant::now());
    *watcher = DirectWatcher::spawn(listener, tx.clone());
}

/// Classifies Direct *state changes* into a delayed single or a double gesture.
/// Scarlett applies Direct in firmware first, so the single action restores its
/// previous state before requesting Windows mute.
struct DirectGesture {
    pending: Option<(bool, Instant)>,
    ignore_until: Instant,
    expected_own_state: Option<(bool, Instant)>,
}

#[derive(Clone, Copy)]
enum DirectAction {
    Single { baseline: bool },
    Double { baseline: bool },
}

impl DirectGesture {
    fn new() -> Self {
        Self {
            pending: None,
            ignore_until: Instant::now(),
            expected_own_state: None,
        }
    }

    fn changed(&mut self, now: Instant, current: bool, window: Duration) -> Option<DirectAction> {
        if self
            .expected_own_state
            .is_some_and(|(expected, deadline)| now <= deadline && current == expected)
        {
            self.expected_own_state = None;
            return None;
        }
        if self
            .expected_own_state
            .is_some_and(|(_, deadline)| now > deadline)
        {
            self.expected_own_state = None;
        }
        if now < self.ignore_until {
            return None;
        }
        if let Some((baseline, _)) = self.pending.take() {
            return Some(DirectAction::Double { baseline });
        }
        self.pending = Some((!current, now + window));
        None
    }

    fn tick(&mut self, now: Instant) -> Option<DirectAction> {
        let (baseline, deadline) = self.pending?;
        if now < deadline {
            return None;
        }
        self.pending = None;
        Some(DirectAction::Single { baseline })
    }

    fn cancel(&mut self) {
        self.pending = None;
    }

    /// A single press is waiting for its double-click window to expire.
    fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    fn suppress_own_notifications(&mut self, now: Instant, expected_state: bool) {
        self.ignore_until = now + Duration::from_millis(200);
        self.expected_own_state = Some((expected_state, now + Duration::from_secs(2)));
    }
}

/// The mute state FocusMute last asked for, until the audio callback reports
/// it. Toggling from the reported state alone sends the same command twice
/// when two presses land before the first is confirmed.
struct MuteIntent {
    target: std::cell::Cell<Option<(bool, Instant)>>,
    /// The last request, kept after the monitor reports it: tells a
    /// transition FocusMute caused from one made elsewhere.
    last: std::cell::Cell<Option<(bool, Instant)>>,
}

impl MuteIntent {
    /// Longer than any observed `SetMute` → callback delay.
    const HOLD: Duration = Duration::from_millis(500);

    /// How long after a request a confirmed transition to the requested
    /// state still counts as its result. Generous on purpose: a slow loop
    /// must not turn FocusMute's own mute into one made elsewhere.
    const CONFIRM: Duration = Duration::from_secs(2);

    fn new() -> Self {
        Self {
            target: std::cell::Cell::new(None),
            last: std::cell::Cell::new(None),
        }
    }

    /// The state to toggle from: the pending request while it is fresh and
    /// unconfirmed, otherwise what the monitor reports. A confirmed request is
    /// forgotten, so a change made elsewhere right after it (browser sync, the
    /// Windows mixer) is not overridden for the rest of [`Self::HOLD`].
    fn current(&self, reported: bool) -> bool {
        match self.target.get() {
            Some((target, at)) if target != reported && at.elapsed() < Self::HOLD => target,
            _ => {
                self.target.set(None);
                reported
            }
        }
    }

    /// Drop the requests: they were made to another microphone.
    fn forget(&self) {
        self.target.set(None);
        self.last.set(None);
    }

    /// Ask the monitor for `muted` and remember the request.
    fn request<M: MuteMonitor + ?Sized>(
        &self,
        monitor: &M,
        muted: bool,
    ) -> focusmute_lib::audio::Result<()> {
        monitor.set_muted(muted)?;
        let now = Instant::now();
        self.target.set(Some((muted, now)));
        self.last.set(Some((muted, now)));
        Ok(())
    }

    /// A transition to `muted` was confirmed: if it answers FocusMute's last
    /// request, consume that request and return how long ago it was made.
    fn confirms(&self, muted: bool) -> Option<Duration> {
        match self.last.get() {
            Some((requested, at)) if requested == muted && at.elapsed() < Self::CONFIRM => {
                self.last.set(None);
                Some(at.elapsed())
            }
            _ => None,
        }
    }
}

/// Take over the mute state of a microphone FocusMute has just started
/// watching (a late-created monitor or a new default microphone). This is not
/// a mute or unmute by the user, so no sound, OSD or hook fires; the LEDs and
/// the tray only follow the new state.
fn adopt_mute_state(
    state: &mut TrayState,
    device: Option<&impl ScarlettDevice>,
    tray_menu: &state::TrayMenu,
    muted: bool,
) {
    if muted == state.indicator.is_muted() {
        return;
    }
    state.indicator.force_state(muted);
    if let Some(dev) = device {
        let result = if muted {
            state.indicator.apply_mute(dev)
        } else {
            state.indicator.clear_mute(dev)
        };
        if let Err(e) = result {
            log::warn!("[mute] could not update LEDs for the new microphone: {e}");
        }
    }
    tray_menu.set_muted(muted);
}

/// Mute state the tray keeps about the microphone being watched. Everything
/// here belongs to one microphone and is dropped by [`Self::switched`].
struct MicTracking {
    /// FocusMute muted this microphone itself: by hotkey, menu, push-to-talk,
    /// Direct or browser sync. Only such a mute is undone on exit; a mute made
    /// elsewhere (Windows, another app), or present before FocusMute watched
    /// the microphone, is left in place.
    owned_mute: bool,
    /// The last mute command, until the monitor confirms it.
    intent: MuteIntent,
    /// The user was told the switch to a new default microphone failed.
    stale_notified: bool,
    /// A push-to-talk press unmuted this microphone; release re-mutes it.
    ptt_active: bool,
    ptt_noop_logged: bool,
    /// A browser-sync command waits for its confirming transition, whose beep
    /// and reverse sync are then suppressed.
    browser_sync_pending: bool,
    /// The log says mute goes to a Scarlett input that is not the Windows
    /// default microphone.
    not_default_logged: bool,
}

impl MicTracking {
    fn new() -> Self {
        Self {
            owned_mute: false,
            intent: MuteIntent::new(),
            stale_notified: false,
            ptt_active: false,
            ptt_noop_logged: false,
            browser_sync_pending: false,
            not_default_logged: false,
        }
    }

    /// The monitor now watches another microphone. A push-to-talk release or
    /// a browser command confirmation would otherwise act on it.
    fn switched(&mut self) {
        self.owned_mute = false;
        self.intent.forget();
        self.ptt_active = false;
        self.browser_sync_pending = false;
    }
}

/// Follow a change of the Windows default microphone: adopt its state
/// silently. Queued samples of the previous one carry an older generation and
/// are dropped where they are drained. `retry_now` retries a failed switch at
/// once, before a command is sent.
fn follow_default_mic<M: MuteMonitor + ?Sized>(
    monitor: &M,
    retry_now: bool,
    state: &mut TrayState,
    device: Option<&impl ScarlettDevice>,
    tray_menu: &state::TrayMenu,
    mic: &mut MicTracking,
) {
    if !monitor.maintain(retry_now) {
        return;
    }
    mic.switched();
    adopt_mute_state(state, device, tray_menu, monitor.is_muted());
}

/// Tell the user once per stale period why a command did nothing.
fn report_stale_mic<M: MuteMonitor + ?Sized>(monitor: &M, mic: &mut MicTracking, language: &str) {
    if monitor.is_stale() && !mic.stale_notified {
        mic.stale_notified = true;
        crate::notification::Notifier::show_oneshot(crate::i18n::tr(
            language,
            "notify_mic_not_ready",
        ));
    }
}

/// What the tray says about the microphone being watched.
fn mic_status<M: MuteMonitor + ?Sized>(monitor: Option<&M>) -> state::MicStatus {
    match monitor {
        None => state::MicStatus::Missing,
        Some(m) => state::MicStatus::Watching {
            name: m.device_name(),
            stale: m.is_stale(),
            not_default: !m.watches_default(),
        },
    }
}

/// Classify one device notification from the Direct listener.
#[allow(clippy::too_many_arguments)]
fn handle_direct_notify<M: MuteMonitor>(
    device: &impl ScarlettDevice,
    panel: &PanelControls,
    monitor: Option<&M>,
    intent: &MuteIntent,
    muted: bool,
    window_ms: u64,
    gesture: &mut DirectGesture,
    packet: &[u8],
) {
    let now = Instant::now();
    let Some(mask) = focusmute_lib::direct::notification_mask(packet) else {
        log::warn!(
            "[direct] ignored short device notification ({} bytes)",
            packet.len()
        );
        return;
    };
    if mask & panel.video_call_notify_mask != 0 {
        // A three-second hold is reserved by Solo for Combine Inputs.
        gesture.cancel();
        return;
    }
    if mask & panel.direct_notify_mask != 0 {
        match focusmute_lib::direct::direct_enabled(device, panel) {
            Ok(current) => {
                if let Some(action) = gesture.changed(
                    now,
                    current,
                    Duration::from_millis(window_ms.clamp(200, 600)),
                ) {
                    apply_direct_action(
                        device, panel, monitor, intent, muted, gesture, action, now,
                    );
                }
            }
            Err(e) => log::warn!("[direct] could not read Direct state: {e}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_direct_action<M: MuteMonitor>(
    device: &impl ScarlettDevice,
    panel: &PanelControls,
    monitor: Option<&M>,
    intent: &MuteIntent,
    muted: bool,
    gesture: &mut DirectGesture,
    action: DirectAction,
    now: Instant,
) {
    let desired_direct = match action {
        DirectAction::Single { baseline } => {
            log::info!("[direct] single gesture → toggle mute");
            baseline
        }
        DirectAction::Double { baseline } => {
            log::info!("[direct] double gesture → toggle Direct");
            !baseline
        }
    };
    if let Err(e) = focusmute_lib::direct::set_direct_enabled(device, panel, desired_direct) {
        log::warn!("[direct] could not set Direct state: {e}");
        return;
    }
    gesture.suppress_own_notifications(now, desired_direct);
    // Through the intent like a hotkey, so a quick hotkey press after the
    // gesture toggles from the new state and the mute counts as FocusMute's.
    if matches!(action, DirectAction::Single { .. })
        && let Some(monitor) = monitor
        && let Err(e) = intent.request(monitor, !muted)
    {
        log::warn!("[direct] could not toggle mute: {e}");
    }
}

/// Whether the muted-talk meters are read on a [`MeterWatcher`] thread with a
/// handle of its own. The Windows driver serves several handles at once (the
/// settings window already opens one). On Linux the USB interface is claimed
/// exclusively and one request is two transfers, so a second handle cannot
/// open and the main one cannot be shared: the reads stay on the main loop.
const METER_THREAD: bool = cfg!(windows);

/// Background reader of the input meters for the muted-talk blink.
///
/// A meter read is a device round trip; done on the main loop it delayed every
/// mute transition queued behind it. The thread opens its own handle, as the
/// settings window does, and exists only while the microphone is muted with
/// the feature on and a Scarlett connected.
struct MeterWatcher {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl MeterWatcher {
    /// Minimum gap before a new reader after one failed.
    const RETRY: Duration = Duration::from_secs(2);

    fn spawn(serial: String, tx: mpsc::Sender<Msg>) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("meter".into())
            .spawn(move || {
                let device = match open_device_by_serial(&serial) {
                    Ok(device) => device,
                    Err(e) => {
                        log::debug!("[blink] meter reader could not open the device: {e}");
                        return;
                    }
                };
                while !thread_stop.load(Ordering::SeqCst) && crate::RUNNING.load(Ordering::SeqCst) {
                    let started = Instant::now();
                    // A failure means the device is gone or busy; the main
                    // loop starts a new reader after RETRY.
                    let Some(levels) = read_meters_logged(&device) else {
                        break;
                    };
                    if tx.send(Msg::Meters(levels)).is_err() {
                        break;
                    }
                    super::wake::wake_main();
                    std::thread::sleep(
                        super::blink::METER_INTERVAL.saturating_sub(started.elapsed()),
                    );
                }
            })
            .ok()?;
        Some(Self { stop, handle })
    }

    /// Ask the thread to exit; it does so after its current read or wait.
    fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// One meter read; a slow or failed read is logged.
fn read_meters_logged(device: &impl ScarlettDevice) -> Option<Vec<u32>> {
    /// A read slower than this is worth a line in the log.
    const SLOW: Duration = Duration::from_millis(50);
    let started = Instant::now();
    let result = focusmute_lib::meter::read_meters(device, focusmute_lib::meter::METER_COUNT);
    let elapsed = started.elapsed();
    if elapsed > SLOW {
        log::warn!("[blink] slow meter read: {} ms", elapsed.as_millis());
    }
    result
        .map_err(|e| log::warn!("[blink] meter read failed: {e}"))
        .ok()
}

/// Start or stop the meter reader to match `wanted`. A reader that ended on
/// its own failed, and the next one waits [`MeterWatcher::RETRY`]; one stopped
/// here (an unmute) does not delay the next mute's reader.
fn sync_meter_watcher(
    watcher: &mut Option<MeterWatcher>,
    last_failure: &mut Option<Instant>,
    wanted: bool,
    serial: &str,
    tx: &mpsc::Sender<Msg>,
) {
    if watcher.as_ref().is_some_and(|w| w.handle.is_finished()) {
        *watcher = None;
        *last_failure = Some(Instant::now());
    }
    if !wanted {
        if let Some(w) = watcher.take() {
            w.stop();
        }
        return;
    }
    if watcher.is_some() || last_failure.is_some_and(|at| at.elapsed() < MeterWatcher::RETRY) {
        return;
    }
    *watcher = MeterWatcher::spawn(serial.to_string(), tx.clone());
}

/// Whether the Scarlett behind `device` is gone. A removal event names no
/// device and may belong to another Focusrite interface, so the handle is
/// asked; a failed read is retried once, since a busy driver can fail one.
fn device_gone(device: &impl ScarlettDevice) -> bool {
    const RETRY_AFTER: Duration = Duration::from_millis(75);
    if focusmute_lib::device::still_present(device) {
        return false;
    }
    std::thread::sleep(RETRY_AFTER);
    !focusmute_lib::device::still_present(device)
}

/// Everything the tray loop owns. One [`step`](Self::step) handles whatever
/// is pending; the settings window runs it too while it is open, so the tray
/// keeps working behind the modal dialog.
struct TrayCore<P: PlatformAdapter> {
    state: TrayState,
    device: Option<PlatformDevice>,
    monitor: Option<Arc<P::Monitor>>,
    monitor_retry: focusmute_lib::reconnect::ReconnectState,
    mic: MicTracking,
    resources: TrayResources,
    tray_menu: state::TrayMenu,
    rx: Receiver<Msg>,
    /// Kept for the on-demand threads (poll, Direct, meters), so poll-thread
    /// death is detected with `JoinHandle::is_finished` rather than a
    /// disconnect.
    tx: mpsc::Sender<Msg>,
    bg_handle: Option<JoinHandle<()>>,
    poll_thread_dead: bool,
    reverse_slot: Arc<super::browser_sync::ReverseSlot>,
    blink: super::blink::BlinkState,
    direct_gesture: DirectGesture,
    direct_watcher: Option<DirectWatcher>,
    direct_last_spawn: Option<Instant>,
    meter_watcher: Option<MeterWatcher>,
    meter_last_failure: Option<Instant>,
    /// Next main-loop meter read where there is no meter thread.
    next_inline_meter: Option<Instant>,
    /// No window exists until the first banner is shown.
    osd: crate::osd::Osd,
    no_mic_notified: bool,
    quit: bool,
}

impl<P: PlatformAdapter> TrayCore<P> {
    /// Handle everything pending. `settings` is `Some(capturing)` while the
    /// settings window runs this step: the window is not opened again, and
    /// hotkey presses do nothing while it records a shortcut.
    fn step(&mut self, settings: Option<bool>) {
        if settings.is_some() {
            P::pump_events_nested();
        }
        self.maintain_monitor();
        self.maintain_device();
        self.sync_watchers();
        if !self.state.config.osd.enabled {
            self.osd.destroy();
        }
        let meters = self.drain_messages();
        self.update_blink(meters);
        if let Some(ref dev) = self.device
            && let Err(error) = self.state.indicator.animate(dev, Instant::now())
        {
            log::debug!("[led] mute blink failed: {error}");
        }
        self.handle_menu_events(settings.is_some());
        self.handle_hotkeys(settings.unwrap_or(false));
    }

    /// Microphone monitor: created late when there was no capture device at
    /// startup, re-bound when Windows picks another default.
    fn maintain_monitor(&mut self) {
        if self.monitor.is_none() && self.monitor_retry.should_attempt() {
            match P::create_monitor(self.state.config.system.scarlett_microphone) {
                Some(m) => {
                    self.monitor_retry.record_success();
                    let m = Arc::new(m);
                    self.mic.switched();
                    adopt_mute_state(
                        &mut self.state,
                        self.device.as_ref(),
                        &self.tray_menu,
                        m.is_muted(),
                    );
                    self.bg_handle = Some(P::spawn_poll_thread(Arc::clone(&m), self.tx.clone()));
                    self.poll_thread_dead = false;
                    self.no_mic_notified = false;
                    self.monitor = Some(m);
                }
                None => self.monitor_retry.record_failure(),
            }
        }
        if let Some(ref m) = self.monitor {
            // Hot-applies the settings option; a change makes the next
            // maintain pick the microphone again.
            m.set_prefer_scarlett(self.state.config.system.scarlett_microphone);
            follow_default_mic(
                &**m,
                false,
                &mut self.state,
                self.device.as_ref(),
                &self.tray_menu,
                &mut self.mic,
            );
            if !m.is_stale() {
                self.mic.stale_notified = false;
            }
            // The tray marks it; a notification on every start would nag a
            // user who set the option for exactly this case.
            if m.watches_default() {
                self.mic.not_default_logged = false;
            } else if !self.mic.not_default_logged {
                self.mic.not_default_logged = true;
                log::info!(
                    "[audio] muting \"{}\", which is not the Windows default microphone",
                    m.device_name().unwrap_or_default()
                );
            }
        }
        self.tray_menu.set_mic(mic_status(self.monitor.as_deref()));
    }

    /// Device removal (`RegisterDeviceNotification`) and reconnection.
    fn maintain_device(&mut self) {
        // An event while no device is open is consumed and ignored.
        if P::check_device_removed()
            && let Some(ref dev) = self.device
        {
            if device_gone(dev) {
                log::warn!("[device] removed (USB hotplug event)");
                self.drop_device();
            } else {
                log::debug!("[device] removal event, but the device still answers");
            }
        }
        // An arriving Focusrite interface makes the next attempt worth trying
        // now rather than after the backoff.
        if P::check_device_arrived() && self.device.is_none() {
            log::debug!("[device] interface arrived; reconnecting now");
            self.state.reset_backoff();
        }
        if self.device.is_none()
            && let Some(new_dev) = self.state.try_reconnect()
        {
            log::info!("[device] reconnected: {}", new_dev.info().device_name);
            self.device = Some(new_dev);
            // Listeners still on the previous handle would hear the old
            // session; let the next sync open them on the new device.
            if let Some(watcher) = self.direct_watcher.take() {
                watcher.stop();
            }
            self.direct_last_spawn = None;
            if let Some(watcher) = self.meter_watcher.take() {
                watcher.stop();
            }
            self.meter_last_failure = None;
            self.tray_menu
                .set_device(self.device.as_ref().map(|d| d.info().device_name.as_str()));
        }
    }

    fn drop_device(&mut self) {
        self.device = None;
        self.tray_menu.set_device(None);
    }

    /// The Scarlett's Direct button and LED Lab controls, if it has them.
    fn panel(&self) -> Option<&'static PanelControls> {
        self.device
            .as_ref()
            .and_then(|d| focusmute_lib::models::panel(d.info()))
            .map(|p| p.controls)
    }

    fn direct_wanted(&self) -> bool {
        self.state.config.hardware_button.direct_button_enabled && self.panel().is_some()
    }

    /// The muted-talk meter reads are wanted (the microphone may be muted).
    fn talk_watch_enabled(&self) -> bool {
        self.state.config.indicator.blink_on_talk && !self.state.config.indicator.blink_while_muted
    }

    /// The mute state commands toggle from.
    fn mute_baseline(&self) -> bool {
        self.monitor
            .as_deref()
            .map_or(self.state.indicator.is_muted(), |m| {
                self.mic.intent.current(m.is_muted())
            })
    }

    /// Direct listener and meter reader threads, and a Direct single press
    /// whose double-click window ran out.
    fn sync_watchers(&mut self) {
        // Direct is an opt-in gesture on models with panel controls (the
        // Solo). Its listener thread exists only while the option is on;
        // notifications arrive as Msg::DirectNotify.
        let direct_wanted = self.direct_wanted();
        sync_direct_watcher(
            &mut self.direct_watcher,
            &mut self.direct_last_spawn,
            direct_wanted,
            self.device.as_ref(),
            &self.tx,
        );
        if !direct_wanted {
            self.direct_gesture.cancel();
        } else if let Some(ref dev) = self.device
            && let Some(panel) = self.panel()
        {
            let now = Instant::now();
            if let Some(action) = self.direct_gesture.tick(now) {
                let muted = self.mute_baseline();
                apply_direct_action(
                    dev,
                    panel,
                    self.monitor.as_deref(),
                    &self.mic.intent,
                    muted,
                    &mut self.direct_gesture,
                    action,
                    now,
                );
            }
        }
        let meters_wanted = METER_THREAD
            && self.talk_watch_enabled()
            && self.state.indicator.is_muted()
            && self.device.is_some();
        sync_meter_watcher(
            &mut self.meter_watcher,
            &mut self.meter_last_failure,
            meters_wanted,
            &self.state.config.system.device_serial,
            &self.tx,
        );
    }

    /// Drain background messages (non-blocking). Returns the latest meter
    /// levels, if any arrived.
    fn drain_messages(&mut self) -> Option<Vec<u32>> {
        let mut browser_mute_target: Option<bool> = None;
        let mut meters = None;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::MutePoll { muted, generation } => self.handle_mute_poll(muted, generation),
                Msg::DirectNotify(packet) => {
                    // Direct actions run in this pass; a resulting mute change
                    // arrives as a MutePoll on a following pass.
                    if self.direct_wanted()
                        && let Some(ref dev) = self.device
                        && let Some(panel) = self.panel()
                    {
                        let muted = self.mute_baseline();
                        handle_direct_notify(
                            dev,
                            panel,
                            self.monitor.as_deref(),
                            &self.mic.intent,
                            muted,
                            self.state.config.hardware_button.direct_double_click_ms,
                            &mut self.direct_gesture,
                            &packet,
                        );
                    }
                }
                Msg::BrowserMute(muted) => {
                    // Coalesce: Meet flickers its mute state during join-screen
                    // render, producing bursts like false→true→false within one
                    // drain pass. is_muted() is callback-cached and lags set_muted,
                    // so acting per-message evaluates every message against the
                    // same stale snapshot and can latch a transient state. Browser
                    // sync is level-triggered — only the last state matters.
                    browser_mute_target = Some(muted);
                }
                Msg::Meters(levels) => meters = Some(levels),
            }
        }
        if !self.poll_thread_dead && self.bg_handle.as_ref().is_some_and(|h| h.is_finished()) {
            log::error!("[audio] monitor thread stopped unexpectedly");
            self.poll_thread_dead = true;
        }
        if let Some(muted) = browser_mute_target {
            if let Some(ref m) = self.monitor
                && m.is_stale()
            {
                follow_default_mic(
                    &**m,
                    true,
                    &mut self.state,
                    self.device.as_ref(),
                    &self.tray_menu,
                    &mut self.mic,
                );
            }
            handle_browser_mute(
                self.monitor.as_deref(),
                muted,
                self.state.indicator.is_muted(),
                &mut self.mic.browser_sync_pending,
            );
        }
        meters
    }

    /// One mute sample: on a confirmed transition, sound, tray, OSD, hooks
    /// and reverse sync react first, the Scarlett's LEDs follow. A slow LED
    /// write then delays only the LEDs.
    fn handle_mute_poll(&mut self, muted: bool, generation: u64) {
        // Queued before a switch: it describes the previous microphone and
        // would pass as a mute or unmute.
        if self
            .monitor
            .as_ref()
            .is_some_and(|m| m.generation() != generation)
        {
            log::debug!("[mute] dropped a poll of the previous microphone");
            return;
        }
        let action = self.state.confirm_mute_poll(muted);
        if action == MonitorAction::NoChange {
            return;
        }
        // A pending flag means this transition originated in the browser.
        let browser_originated = self.mic.browser_sync_pending;
        let suppress_sound =
            browser_originated && self.state.config.sound.suppress_browser_sync_sound;
        self.mic.browser_sync_pending = false;
        let requested_ago = self.mic.intent.confirms(action == MonitorAction::ApplyMute);
        if let Some(ago) = requested_ago {
            log::debug!("[mute] confirmed {} ms after the request", ago.as_millis());
        }
        self.mic.owned_mute =
            action == MonitorAction::ApplyMute && (requested_ago.is_some() || browser_originated);
        // Reverse sync: mirror USER-initiated transitions to the extension.
        // Browser-originated ones are echoes — the meeting page already shows
        // that state.
        if self.state.config.system.browser_sync_reverse && !browser_originated {
            let muted = action == MonitorAction::ApplyMute;
            log::debug!(
                "[sync] reverse action queued: {}",
                if muted { "mute" } else { "unmute" }
            );
            self.reverse_slot.set(muted);
        }
        state::apply_mute_ui(
            action,
            &self.tray_menu,
            &self.state,
            &mut self.resources,
            suppress_sound,
        );
        // The OSD reports the microphone, so it works without the Scarlett too.
        if self.state.config.osd.enabled {
            let kind = if action == MonitorAction::ApplyMute {
                crate::osd::OsdKind::Muted
            } else {
                crate::osd::OsdKind::Live
            };
            self.osd.show(
                kind,
                &self.state.config.system.language,
                &self.state.config.osd.position,
            );
        }
        if let Some(ref dev) = self.device {
            let started = Instant::now();
            let result = self.state.write_indicator(action, dev);
            log::debug!(
                "[led] indicator written in {} ms",
                started.elapsed().as_millis()
            );
            if let Err(e) = result {
                log::warn!("[device] disconnected (communication error: {e})");
                self.drop_device();
            }
        }
    }

    /// Muted-talk blink: blinks the indicator while the meters show a level
    /// over the threshold.
    fn update_blink(&mut self, meters: Option<Vec<u32>>) {
        if self.talk_watch_enabled()
            && self.state.indicator.is_muted()
            && let Some(ref dev) = self.device
        {
            let now = Instant::now();
            let meters = if METER_THREAD {
                meters
            } else if self.next_inline_meter.is_none_or(|at| now >= at) {
                self.next_inline_meter = Some(now + super::blink::METER_INTERVAL);
                read_meters_logged(dev)
            } else {
                None
            };
            let talking = meters.map(|levels| {
                let level = focusmute_lib::meter::max_input_level(
                    &levels,
                    &self.state.indicator.strategy().input_indices,
                );
                level >= self.state.config.indicator.talk_threshold
            });
            if talking == Some(true) && self.state.config.osd.enabled {
                self.osd.show(
                    crate::osd::OsdKind::Talking,
                    &self.state.config.system.language,
                    &self.state.config.osd.position,
                );
            }
            if let Some(action) = self.blink.advance(now, talking) {
                apply_blink_action(dev, &self.state, action);
            }
        } else {
            self.next_inline_meter = None;
            if let Some(action) = self.blink.reset(self.state.indicator.is_muted())
                && let Some(ref dev) = self.device
            {
                apply_blink_action(dev, &self.state, action);
            }
        }
    }

    /// A command is about to go out: if Windows moved the default microphone
    /// and the switch failed, try it now rather than in 2 s.
    fn retry_stale_mic(&mut self) {
        if let Some(ref m) = self.monitor
            && m.is_stale()
        {
            follow_default_mic(
                &**m,
                true,
                &mut self.state,
                self.device.as_ref(),
                &self.tray_menu,
                &mut self.mic,
            );
        }
    }

    fn handle_menu_events(&mut self, in_settings: bool) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match state::menu_outcome(&event, &self.tray_menu) {
                state::MenuOutcome::ToggleMute => {
                    self.retry_stale_mic();
                    if let Some(m) = self.monitor.clone() {
                        let target = !self.mic.intent.current(m.is_muted());
                        if let Err(e) = self.mic.intent.request(&*m, target) {
                            log::warn!("[mute] failed to toggle mute: {e}");
                        }
                        report_stale_mic(&*m, &mut self.mic, &self.state.config.system.language);
                    }
                }
                state::MenuOutcome::OpenSettings if in_settings => {
                    log::debug!("[settings] already open");
                }
                state::MenuOutcome::OpenSettings => self.open_settings(),
                state::MenuOutcome::Reconnect => {
                    self.state.reset_backoff();
                    self.drop_device();
                }
                state::MenuOutcome::Quit => {
                    self.quit = true;
                    return;
                }
                state::MenuOutcome::None => {}
            }
        }
    }

    /// Show the modal settings window. The tray keeps stepping inside it; the
    /// result is applied once it closes.
    fn open_settings(&mut self) {
        log::info!("[settings] dialog opened");
        let config = self.state.config.clone();
        let info = self.device.as_ref().map(|d| d.info().clone());
        let input_count = self
            .state
            .ctx
            .as_ref()
            .and_then(|c| c.input_count())
            .unwrap_or(0);
        let mut tick = |capturing: bool| {
            self.step(Some(capturing));
            !self.quit && RUNNING.load(Ordering::SeqCst)
        };
        let result = crate::settings_dialog::show_settings(
            &config,
            input_count,
            info.as_ref(),
            Some(&mut tick),
        );
        state::apply_settings_result(
            result,
            &mut self.tray_menu,
            &mut self.state,
            self.device.as_ref(),
            &mut self.resources,
        );
    }

    /// `capturing`: the settings window is recording a shortcut, so presses
    /// are the user typing it, not commands.
    fn handle_hotkeys(&mut self, capturing: bool) {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if capturing && event.state == HotKeyState::Pressed {
                continue;
            }
            let Some(m) = self.monitor.clone() else {
                if !self.no_mic_notified && event.state == HotKeyState::Pressed {
                    self.no_mic_notified = true;
                    log::warn!("[hotkey] no microphone to mute yet");
                    crate::notification::Notifier::show_oneshot(crate::i18n::tr(
                        &self.state.config.system.language,
                        "notify_no_mic",
                    ));
                }
                continue;
            };
            // A PTT release counts too: it re-mutes.
            self.retry_stale_mic();
            let language = self.state.config.system.language.clone();
            let mic = &mut self.mic;
            if event.id == self.resources.hotkey.toggle_id {
                // Toggle hotkey: act on press only
                if event.state != HotKeyState::Pressed {
                    continue;
                }
                mic.ptt_active = false;
                mic.ptt_noop_logged = false;
                let target_muted = !mic.intent.current(m.is_muted());
                log::info!(
                    "[hotkey] toggle → {}",
                    if target_muted { "muting" } else { "unmuting" }
                );
                if let Err(e) = mic.intent.request(&*m, target_muted) {
                    log::warn!("[hotkey] failed to set mute state: {e}");
                    report_stale_mic(&*m, mic, &language);
                }
            } else if self
                .resources
                .hotkey
                .ptt_id
                .is_some_and(|id| event.id == id)
            {
                // PTT hotkey: press = unmute (if muted), release = re-mute (if PTT activated)
                match event.state {
                    HotKeyState::Pressed => {
                        if mic.intent.current(m.is_muted()) {
                            mic.ptt_noop_logged = false;
                            log::info!("[ptt] held → unmuting");
                            // Only an unmute that went out is undone on
                            // release; otherwise release would mute a
                            // microphone PTT never touched.
                            match mic.intent.request(&*m, false) {
                                Ok(()) => mic.ptt_active = true,
                                Err(e) => {
                                    log::warn!("[ptt] failed to unmute: {e}");
                                    report_stale_mic(&*m, mic, &language);
                                }
                            }
                        } else if !mic.ptt_noop_logged {
                            mic.ptt_noop_logged = true;
                            log::debug!("[ptt] already unmuted, ignoring press");
                        }
                    }
                    HotKeyState::Released => {
                        if mic.ptt_active {
                            mic.ptt_active = false;
                            log::info!("[ptt] released → re-muting");
                            if let Err(e) = mic.intent.request(&*m, true) {
                                log::warn!("[ptt] failed to re-mute: {e}");
                                report_stale_mic(&*m, mic, &language);
                            }
                        }
                    }
                }
            }
        }
    }

    /// How long the loop may wait for the next event. Short only while
    /// something is timed; otherwise wakes come from messages.
    fn wait_timeout(&self) -> Duration {
        let talk_watch = self.talk_watch_enabled() && self.state.indicator.is_muted();
        let busy = self.direct_gesture.is_pending()
            || self.state.indicator.is_animating()
            || (talk_watch && self.device.is_some());
        if busy {
            return LOOP_BUSY;
        }
        // Without a device or microphone, sleep until the next retry is due
        // instead of waking every 50 ms; a hotplug message wakes it sooner.
        let retry_due = [
            self.device
                .is_none()
                .then(|| self.state.reconnect.time_until_next_attempt()),
            self.monitor
                .is_none()
                .then(|| self.monitor_retry.time_until_next_attempt()),
        ]
        .into_iter()
        .flatten()
        .min();
        retry_due.map_or(LOOP_IDLE, |due| due.clamp(LOOP_BUSY, LOOP_IDLE))
    }

    /// Stop the threads, unmute what FocusMute muted and restore the LEDs.
    fn shutdown(mut self, sync_handle: Option<JoinHandle<()>>) {
        if let Some(watcher) = self.direct_watcher.take() {
            watcher.stop();
        }
        if let Some(watcher) = self.meter_watcher.take() {
            watcher.stop();
        }
        self.osd.destroy();

        // Cleanup — join background threads, unmute, restore LEDs, then drop
        // the monitor. Joining before drop ensures the monitor is dropped on
        // the main thread (important for COM cleanup on Windows).
        log::info!("[focusmute] shutting down");
        RUNNING.store(false, Ordering::SeqCst);

        // Timed join helper — waits up to `timeout` for a thread to finish.
        let timed_join = |handle: JoinHandle<()>, name: &str| {
            let (done_tx, done_rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            });
            if done_rx.recv_timeout(Duration::from_secs(3)).is_err() {
                log::warn!("[focusmute] {name} thread did not exit within 3 s — proceeding");
            }
        };

        if let Some(handle) = self.bg_handle.take() {
            timed_join(handle, "poll");
        }
        if let Some(handle) = sync_handle {
            timed_join(handle, "sync");
        }

        // Undo FocusMute's own mute so the user isn't left silently muted
        // after exit (the LEDs can no longer indicate it). A mute made
        // elsewhere, or present before FocusMute watched the microphone,
        // stays: exiting must not open a microphone someone else closed.
        if let Some(ref monitor) = self.monitor
            && monitor.is_muted()
        {
            if self.mic.owned_mute {
                match monitor.unmute_on_exit() {
                    Ok(()) => log::info!("[mute] unmuted on exit"),
                    Err(e) => log::warn!("[mute] failed to unmute on exit: {e}"),
                }
            } else {
                log::info!("[mute] left muted on exit: FocusMute did not mute it");
            }
        }
        drop(self.monitor.take());

        // Only restore LEDs if we were muted (i.e. we actually changed them).
        // Skipping when live avoids a spurious IOCTL that can fail during shutdown.
        if self.state.indicator.is_muted()
            && let Some(ref dev) = self.device
        {
            self.state.restore_on_exit(dev);
        }
    }
}

/// Shared tray event loop.
///
/// Handles config load, device open, monitor creation, menu/icon build,
/// background thread spawn, and the main event loop.  Platform-specific
/// bits are injected via `P: PlatformAdapter`.
pub fn run_core<P: PlatformAdapter>() -> focusmute_lib::error::Result<()> {
    P::platform_init()?;
    super::wake::register_main_thread();

    // Open device and initialise shared state.
    // If the device isn't connected yet, start with a no-op strategy and
    // let the reconnect loop pick it up later.
    let first_run = Config::is_first_run();
    let (config, parse_warnings) = Config::load_with_warnings();
    for w in &parse_warnings {
        log::warn!("[config] {w}");
    }
    if let Some(config_path) = Config::path() {
        log::info!("[config] {}", config_path.display());
    }
    log_startup_banner(&config);
    let (mut state, device) = match open_device_by_serial(&config.system.device_serial) {
        Ok(dev) => {
            let info = dev.info();
            log::info!(
                "[device] {} (firmware {}, {}{})",
                info.device_name,
                info.firmware,
                info.header_layout,
                info.serial
                    .as_ref()
                    .map(|s| format!(", serial {s}"))
                    .unwrap_or_default(),
            );
            let st = TrayState::init_with_config(config, &dev)?;
            (st, Some(dev))
        }
        Err(e) => {
            log::warn!("[device] not found at startup ({e}) — starting without device");
            (TrayState::init_without_device(config), None)
        }
    };

    // Create audio monitor on the main thread. Without a capture device (an
    // autostart that beats the audio driver) it is retried from the loop.
    let monitor: Option<Arc<P::Monitor>> =
        P::create_monitor(state.config.system.scarlett_microphone).map(Arc::new);
    let mut monitor_retry = focusmute_lib::reconnect::ReconnectState::with_defaults();
    if monitor.is_none() {
        monitor_retry.record_failure();
    }

    // A microphone muted before FocusMute watched it is not FocusMute's mute,
    // so it is left muted on exit (MicTracking::owned_mute starts false).
    let initial_muted = monitor.as_ref().is_some_and(|m| m.is_muted());
    log::info!(
        "[mute] initial state: {}",
        if initial_muted { "muted" } else { "live" }
    );

    if initial_muted && let Some(ref dev) = device {
        state.set_initial_muted(true, dev);
    }

    // Init audio/hotkey resources
    let (resources, sound_warnings) = TrayResources::init(&state.config)?;
    if state.config.system.autostart {
        state::refresh_autostart();
    }

    // Build tray menu and icon
    let (menu, mut tray_menu) = state::build_tray_menu(
        &state.config,
        state::TrayView {
            muted: initial_muted,
            mic: mic_status(monitor.as_deref()),
            device: device.as_ref().map(|d| d.info().device_name.clone()),
        },
    );
    state::build_tray_icon(menu, &mut tray_menu)?;

    // Show startup warnings (parse errors + validation errors + sound errors)
    {
        const MAX_SOUND_BYTES: u64 = 10_000_000;
        let mut all_warnings = parse_warnings;
        all_warnings.extend(sound_warnings);
        // An unverified LED profile has to reach the user, not just the log.
        if let Some(w) = state
            .ctx
            .as_ref()
            .and_then(|c| c.profile)
            .and_then(|p| p.provenance_warning())
        {
            all_warnings.push(w);
        }
        let input_count = state.ctx.as_ref().and_then(|c| c.input_count());
        if let Err(errs) = state.config.validate(input_count, MAX_SOUND_BYTES) {
            for e in &errs {
                let msg = e.to_string();
                log::warn!("[config] {msg}");
                all_warnings.push(msg);
            }
        }
        if !all_warnings.is_empty() {
            state::show_startup_warnings(&state.config.system.language, &all_warnings);
        }
    }

    // First-run welcome notification
    if first_run {
        crate::notification::Notifier::show_oneshot(&crate::i18n::trf(
            &state.config.system.language,
            "notify_first_run",
            &[("hotkey", &state.config.keyboard.hotkey)],
        ));
    }

    // Channel for background → main thread communication.
    let (tx, rx): (mpsc::Sender<Msg>, Receiver<Msg>) = mpsc::channel();

    // Spawn background poll thread
    let bg_handle = if let Some(ref monitor) = monitor {
        Some(P::spawn_poll_thread(Arc::clone(monitor), tx.clone()))
    } else {
        log::warn!("[audio] no microphone yet — retrying in the background");
        None
    };

    // Spawn browser sync listener for extension mute sync. The reverse slot
    // carries user-initiated transitions to the extension's poll requests;
    // it always exists so the reverse-sync toggle hot-applies.
    let reverse_slot = Arc::new(super::browser_sync::ReverseSlot::new());
    let sync_port = state.config.system.browser_sync_port;
    let sync_handle = if sync_port > 0 {
        log::info!("[sync] starting on port {sync_port}");
        Some(super::browser_sync::spawn_sync_thread(
            sync_port,
            tx.clone(),
            Arc::clone(&reverse_slot),
        ))
    } else {
        log::info!("[sync] disabled (port=0)");
        None
    };

    // Register for USB device hotplug notifications (event-driven, no polling).
    P::register_device_notifications();

    let mut core = TrayCore::<P> {
        state,
        device,
        monitor,
        monitor_retry,
        mic: MicTracking::new(),
        resources,
        tray_menu,
        rx,
        tx,
        bg_handle,
        poll_thread_dead: false,
        reverse_slot,
        blink: super::blink::BlinkState::new(),
        direct_gesture: DirectGesture::new(),
        direct_watcher: None,
        direct_last_spawn: None,
        meter_watcher: None,
        meter_last_failure: None,
        next_inline_meter: None,
        osd: crate::osd::Osd::new(),
        no_mic_notified: false,
        quit: false,
    };

    while RUNNING.load(Ordering::SeqCst) && !core.quit {
        // Platform event pump — dispatches WM_DEVICECHANGE (among others),
        // which sets the device-removed flag the step checks.
        P::pump_events();
        core.step(None);
        if core.quit {
            break;
        }
        P::wait_for_events(core.wait_timeout());
    }

    core.shutdown(sync_handle);
    Ok(())
}

/// The configuration summary logged once at start.
fn log_startup_banner(config: &Config) {
    let on_off = |on: bool| if on { "on" } else { "off" };
    let sync_banner = if config.system.browser_sync_port > 0 {
        config.system.browser_sync_port.to_string()
    } else {
        "off".to_string()
    };
    log::info!(
        "[focusmute] v{} starting (hotkey={}, ptt={}, inputs={}, color={}, sound={}, notifications={}, log={}, sync={}, reverse={}, blink={}, mic={})",
        env!("CARGO_PKG_VERSION"),
        config.keyboard.hotkey,
        if config.keyboard.push_to_talk_hotkey.is_empty() {
            "off"
        } else {
            &config.keyboard.push_to_talk_hotkey
        },
        config.indicator.mute_inputs,
        config.indicator.mute_color,
        on_off(config.sound.sound_enabled),
        on_off(config.system.notifications_enabled),
        config.system.log_level,
        sync_banner,
        on_off(config.system.browser_sync_reverse),
        on_off(config.indicator.blink_on_talk),
        if config.system.scarlett_microphone {
            "scarlett"
        } else {
            "default"
        },
    );
    if !config.hooks.on_mute_url.trim().is_empty() || !config.hooks.on_unmute_url.trim().is_empty()
    {
        log::info!(
            "[hooks] on_mute={:?}, on_unmute={:?}",
            config.hooks.on_mute_url,
            config.hooks.on_unmute_url,
        );
    }
}

/// Perform a blink LED action: the off-phase blanks the indicator's number
/// LEDs, the solid phase repaints the regular mute indicator. Failures are
/// debug-logged only — a missed blink frame is cosmetic, and device loss is
/// detected by the regular poll path.
fn apply_blink_action(
    device: &impl ScarlettDevice,
    state: &TrayState,
    action: super::blink::LedAction,
) {
    let result = match action {
        super::blink::LedAction::Solid => state.indicator.apply_mute(device),
        super::blink::LedAction::Off => {
            focusmute_lib::led::blank_mute_indicator(device, state.indicator.strategy())
        }
    };
    if let Err(e) = result {
        log::debug!("[blink] LED update failed: {e}");
    }
}

/// Apply a (coalesced) browser-sync target, then reconcile the suppression flag.
///
/// `committed_muted` is the debouncer's confirmed (displayed) mute state — the
/// synchronous source of truth for whether a UI transition will fire, unlike
/// the monitor's callback-cached `is_muted()` which lags `set_muted`.
///
/// `browser_sync_pending` is armed to suppress the beep of a browser-caused UI
/// transition, and is normally cleared by that transition's confirming
/// `MutePoll` in the main loop. But a *net-zero* browser flicker — Meet's join
/// screen blips mute→unmute across drain passes without the debouncer ever
/// confirming — produces no confirming transition, so the flag would be
/// stranded `true` and silence the next, unrelated, user-initiated mute
/// (possibly minutes later). If the browser's net target already equals the
/// displayed state, there is nothing left to confirm: clear the flag now.
fn handle_browser_mute<M: MuteMonitor>(
    monitor: Option<&M>,
    target: bool,
    committed_muted: bool,
    browser_sync_pending: &mut bool,
) {
    apply_browser_mute(monitor, target, browser_sync_pending);
    if target == committed_muted {
        *browser_sync_pending = false;
    }
}

/// Apply a (coalesced) browser-sync mute state to the audio monitor.
///
/// Called at most once per drain pass with the last `Msg::BrowserMute`
/// state received. The `is_muted()` gate keeps idempotent messages from
/// arming `browser_sync_pending` (which would suppress the beep of the
/// next user-initiated mute); on `set_muted` failure the flag is rolled
/// back for the same reason.
fn apply_browser_mute<M: MuteMonitor>(
    monitor: Option<&M>,
    muted: bool,
    browser_sync_pending: &mut bool,
) {
    let Some(m) = monitor else {
        log::warn!("[sync] browser mute ignored — no audio monitor");
        return;
    };
    if m.is_muted() == muted {
        log::debug!(
            "[sync] browser mute → already {} (no action)",
            if muted { "muted" } else { "unmuted" }
        );
    } else {
        log::info!(
            "[sync] browser mute → {}",
            if muted { "muting" } else { "unmuting" }
        );
        *browser_sync_pending = true;
        if let Err(e) = m.set_muted(muted) {
            log::warn!("[sync] failed to set mute state: {e}");
            *browser_sync_pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use focusmute_lib::audio::AudioError;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Monitor that models the WASAPI backend's callback-cached state:
    /// `is_muted()` keeps returning the construction-time snapshot no matter
    /// what `set_muted` was called with (the real cache only updates when the
    /// COM change callback fires, long after a drain pass has finished).
    /// Records every `set_muted` call for assertion.
    struct LaggyMonitor {
        stale_muted: bool,
        fail_set: bool,
        set_calls: Mutex<Vec<bool>>,
    }

    impl LaggyMonitor {
        fn new(stale_muted: bool) -> Self {
            Self {
                stale_muted,
                fail_set: false,
                set_calls: Mutex::new(Vec::new()),
            }
        }

        fn failing(stale_muted: bool) -> Self {
            Self {
                fail_set: true,
                ..Self::new(stale_muted)
            }
        }

        fn calls(&self) -> Vec<bool> {
            self.set_calls.lock().unwrap().clone()
        }
    }

    impl MuteMonitor for LaggyMonitor {
        fn is_muted(&self) -> bool {
            self.stale_muted
        }

        fn set_muted(&self, muted: bool) -> focusmute_lib::audio::Result<()> {
            self.set_calls.lock().unwrap().push(muted);
            if self.fail_set {
                Err(AudioError::OperationFailed("test failure".into()))
            } else {
                Ok(())
            }
        }

        fn wait_for_change(&self, _timeout: Duration) -> bool {
            false
        }
    }

    /// Two toggles before the audio callback reports the first must send
    /// opposite commands, not the same one twice.
    #[test]
    fn rapid_toggles_follow_the_pending_request() {
        let monitor = LaggyMonitor::new(false);
        let intent = MuteIntent::new();
        for _ in 0..2 {
            let target = !intent.current(monitor.is_muted());
            intent.request(&monitor, target).unwrap();
        }
        assert_eq!(monitor.calls(), vec![true, false]);
    }

    /// Once the monitor reports the requested state the request is done; a
    /// later outside change is then taken as reported.
    #[test]
    fn a_confirmed_request_is_forgotten() {
        let intent = MuteIntent::new();
        intent.request(&LaggyMonitor::new(false), true).unwrap();
        assert!(intent.current(true), "confirmed");
        assert!(!intent.current(false), "changed elsewhere afterwards");
    }

    /// A request to the previous microphone says nothing about the new one:
    /// the next toggle starts from the new microphone's state.
    #[test]
    fn a_switch_forgets_the_pending_request() {
        let intent = MuteIntent::new();
        intent.request(&LaggyMonitor::new(false), true).unwrap();
        intent.forget();
        assert!(!intent.current(false));
    }

    /// Samples carry the generation read before the state, so the main loop
    /// can drop the ones taken before a switch.
    #[test]
    fn mute_samples_carry_the_monitor_generation() {
        struct Switched;
        impl MuteMonitor for Switched {
            fn is_muted(&self) -> bool {
                true
            }
            fn set_muted(&self, _muted: bool) -> focusmute_lib::audio::Result<()> {
                Ok(())
            }
            fn wait_for_change(&self, _timeout: Duration) -> bool {
                false
            }
            fn generation(&self) -> u64 {
                3
            }
        }
        assert!(matches!(
            mute_sample(&Switched),
            Msg::MutePoll {
                muted: true,
                generation: 3
            }
        ));
    }

    /// A confirmed mute that answers FocusMute's request is FocusMute's; the
    /// request is used up, so a later mute made elsewhere is not.
    #[test]
    fn only_a_requested_transition_is_confirmed_as_own() {
        let intent = MuteIntent::new();
        assert!(intent.confirms(true).is_none(), "nothing was requested");
        intent.request(&LaggyMonitor::new(false), true).unwrap();
        assert!(intent.confirms(false).is_none(), "the other direction");
        assert!(intent.confirms(true).is_some());
        assert!(intent.confirms(true).is_none(), "the request is used up");
    }

    /// Reading the toggle baseline must not lose the request: a confirmed
    /// mute is still attributed after `current` forgot the pending target.
    #[test]
    fn confirmation_survives_reading_the_toggle_state() {
        let intent = MuteIntent::new();
        intent.request(&LaggyMonitor::new(false), true).unwrap();
        assert!(intent.current(true), "the monitor reports the mute");
        assert!(intent.confirms(true).is_some());
    }

    /// Nothing tied to the previous microphone survives a switch: a held
    /// push-to-talk must not re-mute the new one on release, a browser
    /// command must not swallow its next beep, and its mute is not ours.
    #[test]
    fn a_switch_drops_the_previous_microphone_state() {
        let mut mic = MicTracking::new();
        mic.owned_mute = true;
        mic.ptt_active = true;
        mic.browser_sync_pending = true;
        mic.intent.request(&LaggyMonitor::new(false), true).unwrap();
        mic.switched();
        assert!(!mic.owned_mute && !mic.ptt_active && !mic.browser_sync_pending);
        assert!(mic.intent.confirms(true).is_none());
    }

    /// A failed command is not remembered: the next toggle starts again
    /// from what the monitor reports.
    #[test]
    fn a_failed_request_leaves_the_reported_state_in_charge() {
        let monitor = LaggyMonitor::failing(false);
        let intent = MuteIntent::new();
        assert!(intent.request(&monitor, true).is_err());
        assert!(!intent.current(monitor.is_muted()));
    }

    /// Replay a burst through the drain-loop coalescing (last state wins)
    /// and the post-drain apply, exactly as `run_core` does.
    fn process_burst(monitor: &LaggyMonitor, burst: &[bool], pending: &mut bool) {
        let mut target: Option<bool> = None;
        for &muted in burst {
            target = Some(muted);
        }
        if let Some(muted) = target {
            apply_browser_mute(Some(monitor), muted, pending);
        }
    }

    // ── Burst coalescing (regression for the Meet join-screen flicker) ──

    /// The logged incident: muted user joins, Meet flickers false→true→false.
    /// Exactly one unmute must fire — the transient "muted" must not act.
    #[test]
    fn flicker_burst_while_muted_unmutes_once() {
        let monitor = LaggyMonitor::new(true);
        let mut pending = false;
        process_burst(&monitor, &[false, true, false], &mut pending);
        assert_eq!(monitor.calls(), vec![false]);
        assert!(pending);
    }

    /// The latch-up case the pre-coalescing code got wrong: user already
    /// unmuted, same flicker burst. Per-message handling muted on the
    /// transient "true" and then dropped the correcting "false" because the
    /// stale cache still read unmuted — leaving the interface muted while
    /// Meet showed unmuted. Coalescing must produce no action at all.
    #[test]
    fn flicker_burst_while_unmuted_is_noop() {
        let monitor = LaggyMonitor::new(false);
        let mut pending = false;
        process_burst(&monitor, &[false, true, false], &mut pending);
        assert!(monitor.calls().is_empty());
        assert!(!pending);
    }

    /// Burst ending in a real state change: only the final state acts.
    #[test]
    fn flicker_burst_ending_muted_mutes_once() {
        let monitor = LaggyMonitor::new(false);
        let mut pending = false;
        process_burst(&monitor, &[true, false, true], &mut pending);
        assert_eq!(monitor.calls(), vec![true]);
        assert!(pending);
    }

    #[test]
    fn empty_drain_does_nothing() {
        let monitor = LaggyMonitor::new(true);
        let mut pending = false;
        process_burst(&monitor, &[], &mut pending);
        assert!(monitor.calls().is_empty());
        assert!(!pending);
    }

    // ── Single-message gate semantics (preserved from the v0.9.1 beep fix) ──

    #[test]
    fn idempotent_message_does_not_arm_suppression_flag() {
        let monitor = LaggyMonitor::new(true);
        let mut pending = false;
        apply_browser_mute(Some(&monitor), true, &mut pending);
        assert!(monitor.calls().is_empty());
        assert!(!pending);
    }

    #[test]
    fn state_change_arms_suppression_flag_and_sets_mute() {
        let monitor = LaggyMonitor::new(false);
        let mut pending = false;
        apply_browser_mute(Some(&monitor), true, &mut pending);
        assert_eq!(monitor.calls(), vec![true]);
        assert!(pending);
    }

    #[test]
    fn set_muted_failure_rolls_back_suppression_flag() {
        let monitor = LaggyMonitor::failing(true);
        let mut pending = false;
        apply_browser_mute(Some(&monitor), false, &mut pending);
        assert_eq!(monitor.calls(), vec![false]);
        assert!(!pending);
    }

    #[test]
    fn missing_monitor_is_noop() {
        let mut pending = false;
        apply_browser_mute::<LaggyMonitor>(None, true, &mut pending);
        assert!(!pending);
    }

    // ── Net-zero flicker reconciliation ──
    //
    // `handle_browser_mute` clears `browser_sync_pending` when the browser's
    // net target already equals the committed (displayed) state, because no
    // confirming `MutePoll` transition will arrive to clear it otherwise.

    /// Replay a sequence of per-drain-pass browser targets through the full
    /// post-drain handling (apply + reconciliation) against a fixed committed
    /// UI state — one call per drain pass, as the main loop does.
    fn drive_drain_passes(
        monitor: &LaggyMonitor,
        committed_muted: bool,
        per_pass_targets: &[bool],
        pending: &mut bool,
    ) {
        for &target in per_pass_targets {
            handle_browser_mute(Some(monitor), target, committed_muted, pending);
        }
    }

    /// The incident this guards against: an unmuted user's Meet join flickers
    /// mute→unmute across two drain passes. The debouncer never confirms the
    /// blip, so the committed state stays unmuted and no `MutePoll` clears the
    /// flag. The flag must not survive the burst, or the next user mute is
    /// silent. Cache lags at unmuted throughout (worst case).
    #[test]
    fn net_zero_flicker_does_not_strand_pending() {
        let monitor = LaggyMonitor::new(false);
        let mut pending = false;
        drive_drain_passes(&monitor, false, &[true, false], &mut pending);
        assert!(
            !pending,
            "net-zero browser flicker stranded the suppression flag"
        );
    }

    /// A genuine browser mute (committed UI still unmuted, a real transition
    /// pending) must KEEP the flag armed so the confirming `MutePoll`'s beep
    /// is suppressed — reconciliation must not clear it prematurely.
    #[test]
    fn genuine_browser_mute_keeps_pending_until_confirmed() {
        let monitor = LaggyMonitor::new(false);
        let mut pending = false;
        handle_browser_mute(Some(&monitor), true, false, &mut pending);
        assert!(
            pending,
            "genuine browser transition must keep the suppression flag armed"
        );
    }

    /// Once the displayed state catches up to the browser target (transition
    /// confirmed), a repeat report at the now-matching state self-heals the
    /// flag rather than leaving it armed for a later unrelated mute.
    #[test]
    fn target_matching_committed_clears_pending() {
        let monitor = LaggyMonitor::new(true);
        let mut pending = true;
        handle_browser_mute(Some(&monitor), true, true, &mut pending);
        assert!(!pending);
    }

    /// Monitor whose first wait reports a mute event; later waits time out.
    struct EventOnceMonitor {
        fired: std::sync::atomic::AtomicBool,
    }

    impl MuteMonitor for EventOnceMonitor {
        fn is_muted(&self) -> bool {
            true
        }
        fn set_muted(&self, _muted: bool) -> focusmute_lib::audio::Result<()> {
            Ok(())
        }
        fn wait_for_change(&self, timeout: Duration) -> bool {
            if self.fired.swap(true, Ordering::SeqCst) {
                std::thread::sleep(timeout);
                false
            } else {
                true
            }
        }
    }

    /// The debouncer needs two equal samples. After an event the second one
    /// must follow within POLL_CONFIRM, not a whole POLL_IDLE later.
    #[test]
    fn poll_loop_confirms_an_event_without_waiting_the_idle_period() {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let monitor = EventOnceMonitor {
                fired: std::sync::atomic::AtomicBool::new(false),
            };
            run_poll_loop(&monitor, &tx);
        });
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let gap = start.elapsed();
        assert!(matches!(first, Msg::MutePoll { muted: true, .. }));
        assert!(matches!(second, Msg::MutePoll { muted: true, .. }));
        assert!(gap < POLL_IDLE, "confirming sample took {gap:?}");
        drop(rx);
        handle.join().unwrap();
    }

    #[test]
    fn direct_gesture_reports_pending_single_press() {
        let mut gesture = DirectGesture::new();
        assert!(!gesture.is_pending());
        let now = Instant::now() + Duration::from_secs(1);
        assert!(
            gesture
                .changed(now, true, Duration::from_millis(350))
                .is_none()
        );
        assert!(gesture.is_pending());
        gesture.cancel();
        assert!(!gesture.is_pending());
    }
}
