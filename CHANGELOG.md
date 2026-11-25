# FocusMute Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.3.1] - 2026-11-25

### Fixed

- Running the test suite no longer overwrites the user's config file with test values (a mute sound path of `/some/new/path.wav`, the F12 hotkey and Ctrl+Space push-to-talk)

## [1.3.0] - 2026-11-24

Fixes from the reviews in docs/25.1 and docs/25.2.

### Added

- The tray menu and tooltip say when "Always mute the Scarlett microphone" mutes an input that is not the Windows default microphone
- Settings report a custom sound file that is not a valid WAV when it is chosen, played or saved, instead of silently playing the built-in sound

### Changed

- The mute sound, tray icon, OSD and hooks react before the Scarlett's LEDs are written, so a slow device no longer delays the sound
- On Windows the muted-talk blink reads the input meters on its own thread, so a slow read no longer delays mute and unmute
- The tray keeps working while the settings window is open: hotkeys, push-to-talk, the Direct button, the tray menu and device reconnection act at once, with sound and LEDs, instead of after the window closes
- The log is appended across restarts; past 1 MB it is moved to `focusmute.old.log` on start
- A device-removal event drops the Scarlett only if it still does not answer after a retry

### Fixed

- Exit unmuted a microphone that was muted elsewhere (Windows, another app) after FocusMute started; only FocusMute's own mute is undone now
- Releasing push-to-talk after the default microphone changed muted the new microphone
- A browser-sync command pending across a microphone switch silenced the next mute's sound
- The Direct gesture's mute bypassed the double-press protection of the hotkey
- `focusmute-cli status`, `mute` and `unmute` ignored `--config` when choosing the microphone
- The `[sound]` log line claimed playback started when `PlaySound` failed
- A reverse-sync test could fail on a coarse clock

## [1.2.0] - 2026-11-23

Ports barnumbirr/focusmute 0.12.0 and 0.12.1 (per-device LED offsets, the
Solo profile, firmware 3.x headers, dependency updates) and fixes from the
Windows review in docs/23 and docs/24.

### Added

- Scarlett Solo 4th Gen model profile, verified on this project's device: numbers 4 and 12, Output 24–25, offsets from the Solo's own firmware schema. The Solo no longer reads its schema from the device at startup
- Firmware 3.x support: the descriptor header is read in one request and recognised in both the 2.x and 3.x layouts. On 3.x the LED offsets come from the device schema; the device is refused rather than written at 2.x offsets if the schema cannot be read. `status`, About and the log show the layout next to the firmware version
- The tray tooltip names the microphone the mute applies to, and says when the Scarlett is disconnected
- The tray menu shows the mute state, the microphone and the connected Scarlett on their own lines. "No microphone" and a pending switch to a new default microphone are shown in the menu and the tooltip
- Settings → System → "Always mute the Scarlett microphone" (`system.scarlett_microphone`, Windows): mute goes to the Focusrite input even when Windows uses another microphone by default, falls back to the default microphone while the Scarlett is unplugged and returns to it when it is back. `focusmute-cli mute`, `status` and `monitor` follow the same option

### Changed

- LED code knows no model names: offsets, number LEDs, the `extended` Output indicator and the Direct/LED Lab controls all come from the model profile. Every Solo and 2i2 device write is checked byte for byte against a snapshot recorded on 1.1.0
- On firmware 3.x the Solo's Direct gesture and LED Lab are off until a 3.x schema is verified; mute indication keeps working
- Sound, OSD, notification and hooks follow the Windows microphone even with the Scarlett unplugged
- The OSD appears on the monitor of the foreground window, at that monitor's DPI
- A microphone that was already muted when FocusMute started watching it stays muted on exit
- tray-icon 0.26, muda 0.21, rodio 0.23, dirs 7. The Windows build no longer links cpal: rodio there only decodes custom WAV files. libxdo is no longer needed on Linux

### Fixed

- `focusmute-cli map` on a Solo wrote the LED mode to the 2i2's offset 77, which on the Solo is `directMonitorAltOffset`; it now uses the schema's 72
- Without a capture device at startup (autostart before the audio driver) FocusMute stayed without a microphone until restart; the monitor is now retried, and a failed switch to a new default microphone is retried too
- Two hotkey presses before the first was confirmed sent the same command twice
- A Focusrite arrival now triggers an immediate reconnect. A removal event is checked against the open handle (its descriptor header must still name the device), so an event for another interface or an earlier session no longer drops a working handle, and a real unplug during a reconnect is still caught. A context failure after opening keeps the reconnect backoff. Without a device the loop sleeps until the next attempt instead of waking every 50 ms
- When Windows moves the default microphone and FocusMute cannot open it yet, commands are refused instead of muting the microphone that is no longer the default; a hotkey, the tray toggle or browser sync retries the switch at once, and the user is told why if it still fails. Exit still unmutes the microphone FocusMute muted. Samples of the previous microphone are told apart by a generation counter, so a toggle right after a switch shows at once, and a pending toggle does not carry over to the new microphone
- A shortcut that fails to register is not saved, and a failed push-to-talk change keeps the old one
- The tray tooltip and all desktop notifications follow the interface language
- With autostart on, the login entry follows a moved portable EXE
- udev rules file: removed a UTF-8 BOM and a joined comment line

## [1.1.0] - 2026-11-21

### Added

- Built-in electronic mute/unmute sound presets, synthesized in memory: `chip` (default), `blip`, `arcade`, `sweep`, `pulse`; the original WAV pair stays available as `classic`. New `[sound].sound_preset` field and a "Sound set" selector in Settings that auditions the choice immediately. A custom WAV path still overrides the preset for that event
- Optional on-screen indicator (Windows, `[osd]`, off by default): a click-through banner for 1.5 s when mute changes, anchored to a corner or the bottom center. With "Blink on talk" enabled it also warns when you talk while muted. While off, no window, timer or thread exists
- The tray follows a change of the Windows default microphone instead of staying on the device it started with

### Changed

- Mute feedback (LEDs, sound, icon) follows the hotkey in ~30 ms instead of ~280 ms: after a mute event the poll thread takes its confirming sample after 25 ms rather than after the 250 ms idle period, and background threads wake the main loop directly. The idle main loop now wakes every 500 ms instead of every 50 ms
- The Direct button is watched by a background listener that exists only while the option is on; the main loop no longer issues a notification IOCTL 20 times a second
- Windows plays mute/unmute sounds with `PlaySoundW` through the system mixer, without opening an audio stream per beep
- The settings window repaints at 100 ms instead of 30 ms except while capturing a shortcut
- Solo LED documentation: LED 5 is the first segment of the input 1 level ring (5–11). Channel numbers are unchanged: input 1 is LED 4, input 2 is LED 12

### Fixed

- CI clippy failures (`-D warnings`): `chunks_exact(4)` in the LED lab is now `as_chunks::<4>()`, and `volume_row` takes its preview source as one argument

## [0.11.0] - 2026-09-11

### Changed

- Dependency refresh: GUI stack on egui/eframe 0.36, `toml` 1.0, plus `flate2` and `log` patches. MSRV is now rustc 1.95
- The reverse-sync setting is labeled "Let FocusMute mute/unmute the meeting"; it covers Microsoft Teams as well as Google Meet

### Fixed

- `webbrowser`, reached through the settings dialog's GUI stack, could have extra browser flags injected into it through the Unix `BROWSER` environment variable (RUSTSEC-2026-0257)

## [0.10.0] - 2026-08-11

### Added

- Blink the mute indicator when you talk while muted — opt-in via `[indicator].blink_on_talk`, with Low/Medium/High sensitivity presets in the settings dialog (raw `talk_threshold` remains available in the config file); reads the device's live input meters (~11 Hz) only while muted
- Reverse browser sync — opt-in via `[system].browser_sync_reverse`: hotkey/tray mute changes are mirrored into browser meetings (the extension clicks Meet's/Teams' mute button). New `poll_actions` message on the browser sync listener; browser-originated transitions are excluded to prevent echo loops

### Changed

- Dependency refresh across the tree: GUI stack on egui/eframe 0.34, tray-icon 0.24, muda 0.19, global-hotkey 0.8; Linux USB stack on nusb 0.2; toml 0.9, base64 0.23, auto-launch 0.6, winreg 0.56, notify-rust 4.18
- `[system].websocket_port` renamed to `browser_sync_port` (the listener has been plain HTTP since 0.9.1). Old key still read; configs rewrite to the new name on next save
- MSRV: rustc 1.92, verified against that exact toolchain (1.85 was declared, but let-chains and the egui 0.34 stack require newer)

### Fixed

- The Linux `.deb` package did not install correctly: its maintainer scripts failed and the bundled udev rules were ignored (Windows line endings in the shipped files)
- A mute-state notification whose in-place update fails (e.g. after the notification daemon restarts) is re-shown as a fresh notification instead of failing silently from then on

## [0.9.1] - 2026-08-10

### Added

- Browser extension mute sync — detects mute state from Google Meet and relays it to FocusMute over localhost HTTP, updating Scarlett LEDs to reflect browser mute state
- `websocket_port` config field in `[system]` section (default 0 = disabled for new installs; existing configs without the field get 9736 for backward compatibility); privileged ports (1–1023) rejected by validation
- HTTP listener thread (`127.0.0.1` only) with JSON protocol: `mute_state` POST messages drive `set_muted()`, `ping`/`pong` keepalive; uses plain HTTP instead of WebSocket because Firefox forces TLS on WebSocket connections from extension background scripts. `mute_state` bursts arriving within one event-loop pass are coalesced (last state wins) so transient mute flickers on Meet's join screen cannot latch a stale state
- CORS origin checking: requests from web pages are rejected (403); only browser extension origins (`moz-extension://`, `chrome-extension://`) and origin-less requests are allowed
- HTTP request hardening: case-insensitive header parsing, request line size limit (8 KB), total header size limit (8 KB), `Connection: close` on all responses
- Browser sync settings in Settings > Advanced with port field (restart required on change)
- `suppress_browser_sync_sound` config field (`[sound]`, default `true`) with settings checkbox — skips the mute/unmute beep when the state change was initiated by the browser extension
- Cross-browser extension (Firefox 120+, Chrome/Edge 116+, Manifest V3): content script with MutationObserver on Meet mute button, service worker using `fetch()` HTTP POST
- `Msg::BrowserMute(bool)` event variant for browser-driven mute changes
- `websocket_port_changed` field in `SettingsChanges` with restart notification

### Changed

- Mute/unmute sounds no longer hold an audio output stream open between beeps — each beep opens the stream on a short-lived thread, drains, and closes it. A stream held idle for hours was force-evicted when other apps (games, conferencing) reconfigured the audio device, which could wedge USB DAC drivers until a manual disable/enable cycle. The drain wait is bounded by the sound's duration plus a safety margin, so an audio device lost mid-beep cannot leak the playback thread
- Renamed `websocket.rs` module to `browser_sync.rs`; log prefix `[ws]` → `[sync]`; UI label "WebSocket port" → "Port" (in Browser sync section)
- Startup log banner includes `sync=<port|off>`
- Background thread shutdown uses shared `timed_join` helper (was duplicated inline)
- Channel sender no longer held by main thread — background thread death is now properly detected via `TryRecvError::Disconnected`
- `BrowserMute` messages log a warning when no audio monitor is available (was silently dropped)
- Hooks are now pure webhooks (HTTP POST via `minreq`) — shell command execution removed. `on_mute_command`/`on_unmute_command` renamed to `on_mute_url`/`on_unmute_url` (old names accepted as aliases). New `on_mute_body`/`on_unmute_body` fields for custom JSON payloads (default: `{"event":"mute/unmute"}`)

### Fixed

- Settings dialog Save/Cancel buttons are now readable in Windows light theme — Cancel used a hardcoded dark fill with theme-colored (near-black) text; it now uses the theme's default button styling, and Save's blue fill gets explicit white text

## [0.8.0] - 2026-03-16

### Added

- Push-to-talk hotkey — independent `push_to_talk_hotkey` config field (hold to unmute, release to re-mute), works simultaneously with the toggle hotkey; PTT is a no-op when already unmuted
- Push-to-talk hotkey field in settings dialog with validation (syntax check, duplicate detection)
- Configurable log level in Settings > System (error, warn, info, debug, trace); takes effect on next launch

### Changed

- `get_descriptor` uses `checked_add(8)` to guard against integer overflow on both Windows and Linux
- Schema cache writes are now atomic (temp file + rename), matching the config save pattern
- Sound samples stored in `Arc<Vec<f32>>` for cheap sharing (was `Vec<i16>`)
- Silent `.ok()` error paths now log warnings (schema extraction, layout prediction, per-input color parsing)
- `// SAFETY:` comments on all unsafe blocks (ctrl_handler, device_wndproc, dark mode transmutes, console APIs)
- `SettingsChanges` struct replaces tuple return from `handle_settings_result`

### Infrastructure

- rodio 0.20 → 0.22 (Sink→Player, OutputStream→MixerDeviceSink, i16→f32 samples, NonZero types)
- egui/eframe 0.31 → 0.33
- tray-icon 0.19 → 0.21, muda 0.15 → 0.17, global-hotkey 0.6 → 0.7
- windows 0.61 → 0.62, rfd 0.15 → 0.17

## [0.7.4] - 2026-03-14

### Added

- Disconnected tray icon — grey desaturated icon with "Disconnected" tooltip when no Scarlett device is connected
- Event-driven USB hotplug detection — uses `RegisterDeviceNotificationW` (same approach as Focusrite Control 2) for instant disconnect/reconnect detection on Windows
- First-run welcome notification with hotkey reminder and settings hint
- Settings dialog hint text on hotkey field ("e.g. Ctrl+Shift+M"), color field ("#FF0000 or red"), and tooltip on Mute Inputs
- Hotkey registration failure notification at startup and when changing hotkey in settings
- `FirmwareVersion::is_zero()` method with warning when firmware version defaults to 0.0.0.0
- `is_device_path_present()` lightweight device enumeration (PAL interface only, no serial lookup)

### Changed

- Settings dialog resizable in width only — height auto-adjusts to content (Advanced/About sections)
- Mute UI (icon, sound, notifications) suppressed when device is disconnected — audio interface is unplugged so input states are meaningless
- User-friendly error messages: "Invalid hotkey. Examples: Ctrl+Shift+M, Alt+F1" (was raw syntax error), "Settings file corrupted. Using defaults." (was TOML error detail), "Sound file too large. Maximum size is 10 MB." (was raw byte count)
- Disconnected status shows "Connect a Scarlett 4th Gen device" instead of "Disconnected"
- Init sequence IOCTLs (USB_INIT, GET_CONFIG) use 5-second timeout instead of infinite wait
- Config temp file uses PID-based name in parent directory for robust atomic writes
- Shutdown skips LED restore when app was in live state (avoids spurious IOCTL error)
- Shutdown background thread join uses 3-second timeout instead of blocking indefinitely

### Fixed

- USB response parsing no longer panics on malformed responses — returns error instead of unwrapping
- CLI monitor logs LED apply errors instead of silently discarding them

## [0.7.3] - 2026-03-13

### Changed

- Warn-level logging for previously silent failures: LED apply/clear errors, hotkey registration failures, menu construction errors, and audio output acquisition failures

## [0.7.2] - 2026-03-12

### Fixed

- Sound playback survives sleep/wake cycles — audio output stream is now re-acquired on each mute toggle instead of being held from startup, preventing silent failure when WASAPI endpoints become stale after suspend/resume

## [0.7.1] - 2026-03-09

### Fixed

- System tray context menu now respects Windows dark mode — uses undocumented `uxtheme.dll` APIs (`SetPreferredAppMode`, `FlushMenuThemes`) to opt into dark context menus, matching behavior of Chrome, Firefox, and Windows Terminal

### Infrastructure

- Dependency lockfile bumped (all patch-level updates)

## [0.7.0] - 2026-03-02

### Added

- Structured logging for debugging and issue reports: startup banner with version and config summary, device info on connect/reconnect, mute state changes, settings dialog open/save/cancel, audio monitor readiness, shutdown sequence, and hook completion (debug level)
- Device disconnect logged when communication error causes device loss in tray app
- Config file path logged at tray startup
- Consistent `[tag]` prefixes on all log messages (`[audio]`, `[cli]`, `[config]`, `[device]`, `[focusmute]`, `[hotkey]`, `[layout]`, `[mute]`, `[schema]`, `[settings]`, `[sound]`)
- Linux notification icon — `notify-rust` notifications now display the FocusMute icon (parity with Windows AUMID branding)
- `predict` code generation now emits button labels from the predicted layout instead of an empty TODO placeholder
- Testable `_with`/`_at` variants of `try_reopen`, `try_reconnect_and_refresh`, `save_cache`, and `extract_or_cached` for dependency-injected unit testing
- ~13 new tests: reconnect with mock device factory (6), schema save/load/extract caching workflow (5), code generation button labels (2)

### Changed

- Schema cache includes format version — stale caches from older versions are automatically re-extracted
- Windows FFI: RAII wrappers (`OwnedHandle`, `DevInfoHandle`) replace manual `CloseHandle`/`SetupDiDestroyDeviceInfoList` calls; three duplicated IOCTL functions unified into `ioctl_impl`
- `save_cache` delegates to `save_cache_to`; `extract_or_cached` delegates to `extract_or_cached_at` (DRY)
- Windows notifications use direct WinRT API (`ToastNotification`) instead of `notify-rust`, eliminating a cross-platform abstraction layer on Windows
- `notify-rust` dependency moved to Linux-only

### Fixed

- Settings dialog window icon now uses the same 32 px ICO entry as the tray icon — the "ff" crossbar is visible at titlebar size (previously used the 256 px PNG which lost detail when downscaled)
- Rapid mute/unmute no longer stacks toast notifications — mute-state toasts replace the previous one (Windows: WinRT `SetTag()` replacement, Linux: D-Bus `replaces_id` via `notify-rust`)
- Toast notifications on Windows are now silent (`<audio silent="true" />`) to avoid doubling with FocusMute's own sound feedback

### Infrastructure

- CI check logic extracted into reusable workflow (`check.yml`) shared by CI and Release — eliminates copy-paste drift between the two pipelines
- Fixed missing `libxkbcommon-dev` and `libegl-dev` apt packages in Release workflow (drifted from CI)
- Third-party GitHub Actions pinned by commit SHA; release tag pattern tightened to semver (`v[0-9]+.[0-9]+.[0-9]+*`)
- Clippy now lints all targets (`--all-targets`) including test code
- CI concurrency group cancels superseded runs on the same branch/PR
- Removed SignPath Foundation code signing steps from release workflow (application declined — project too early-stage for their community adoption requirements)

## [0.6.0] - 2026-03-01

### Added

- Notifications toggle in settings dialog (Settings > System > Desktop notifications)
- `config --edit` CLI flag to open config file in `$VISUAL` / `$EDITOR` (falls back to `notepad`/`vi`)
- Per-sound volume sliders in settings dialog (separate volume for mute and unmute sounds)

### Changed

- Sound volume control split into per-sound volumes (`mute_sound_volume`, `unmute_sound_volume`) with separate sliders in settings dialog
- Settings dialog hooks tooltip: curl example rendered in monospace

## [0.5.0] - 2026-03-01

### Added

- `monitor --on-mute <cmd>` and `--on-unmute <cmd>` CLI flags to override hook commands without editing the config file
- Backward-compatible loading of legacy flat config files (pre-v0.5.0)
- Sound loading warnings (missing file, invalid WAV) now surfaced in tray startup notification balloon
- `color_to_rgb` and `rgb_to_hex` public API in `led::color` (DRY consolidation from settings dialog)
- MSRV declared as Rust 1.85 in both crates
- ~20 new tests: audio concurrency, schema edge cases, LED error propagation, settings validation, CLI integration (JSON output, hook flags)

### Changed

- Config file restructured into nested TOML sections (`[indicator]`, `[keyboard]`, `[sound]`, `[system]`, `[hooks]`) — existing flat configs are automatically migrated on next save
- Rich config file header with platform-specific paths and usage notes
- Settings dialog section renamed from "Hotkey" to "Keyboard"
- Settings dialog hooks section now has a labeled header with info tooltip; labels simplified to "On mute" / "On unmute"
- "Reconnect device" tray menu item is now always enabled — shows "Reconnect device" when disconnected and "Refresh device" when connected
- Audio monitor creation failures now logged as warnings on both Windows and Linux (previously silent)
- Release CI now runs all tests (`cargo test`) instead of just lib tests (`cargo test --lib`)

### Fixed

- Hotkey re-registration no longer loses the old hotkey if the new hotkey string is invalid — new hotkey is parsed first, and if registration fails the old hotkey is re-registered as a fallback
- Settings dialog validation errors (red text) now clear when any form field changes (text, color picker, combobox, checkboxes, browse/clear buttons), and the window resizes to keep Cancel/Save buttons visible
- Tray status text no longer overwritten from "Muted" to "Live" when starting with a device already connected in muted state

## [0.4.0] - 2026-03-01

### Added

- `--config <path>` global CLI flag to load settings from a custom TOML file instead of the default location
- Hotkey syntax validation in settings dialog — invalid hotkey strings (e.g. "Ctrl+Blah") now show an error before saving
- "Advanced" collapsible section in settings dialog with "Hooks" subsection (`on_mute_command`, `on_unmute_command`) and info tooltip
- Sound preview "Play" buttons in settings dialog — preview mute/unmute sounds without closing the dialog
- Sound path "Clear" buttons in settings dialog — clear a custom sound path to revert to the built-in sound
- "(built-in)" hint text on empty sound path fields in settings dialog
- `SoundPreviewPlayer` for lazy-initialized audio playback in the settings dialog
- `build_and_validate_config()` pure function extracted from settings dialog save logic for testability (7 unit tests)
- Fatal tray errors displayed as a Windows MessageBox (tray binary has no console)
- Audio poll thread death detection — logs error once if the background mute polling thread stops unexpectedly

### Changed

- Settings dialog "Mute Indicator" section: "Mute Inputs" row now appears above "Mute Color" row
- Settings dialog "Mute Color" text field now fills the full width of the section (right-to-left layout)
- Settings dialog sound rows now fill the full width of the section (right-to-left layout, no fixed button width budget)
- Unmute all inputs on exit — when FocusMute quits, inputs are unmuted so the user isn't left silently muted with no LED indication (applies to both tray app and CLI monitor)
- `set_muted()` errors in tray hotkey and menu toggle handlers now logged as warnings instead of silently ignored

### Fixed

- `enumerate_devices_windows()` now populates device serial by calling the extracted `find_usb_serial()` (previously always returned empty serial in device enumeration)

## [0.3.0] - 2026-02-28

### Added

- `--verbose` / `-v` global CLI flag for debug-level logging
- Config `save_to()` / `load_from()` methods for arbitrary file paths
- Config `load_with_warnings()` method returns parse errors as warnings instead of silently falling back to defaults
- `Config::log_path()` for platform-specific log file location
- Tray app logs to `focusmute.log` in the config directory (info level by default)
- Startup config validation with desktop notification — shows parse errors and validation warnings (invalid colors, out-of-range inputs) as a notification on launch, regardless of `notifications_enabled` setting
- `input_colors` validation in `Config::validate()` — catches invalid color values, out-of-range keys, and non-numeric keys
- Hook command RAII guard (`HookGuard`) — ensures `HOOK_RUNNING` flag is reset even if the hook thread panics
- CLI integration tests for all subcommands (devices, status, mute/unmute/descriptor/probe/monitor/map --help)
- Hook command execution tests (mute and unmute dispatch with marker file verification)

### Changed

- Split `tray/state.rs` (1249 LOC) into submodules: `state/mod.rs`, `state/icon.rs`, `state/menu.rs`, `state/hotkey.rs`
- `Config::save()` now delegates to `Config::save_to()` (DRY refactor)
- `Config::load()` now delegates to `Config::load_with_warnings()` (DRY refactor)
- `CONFIG_HEADER` constant hoisted to module level and shared between save methods

### Fixed

- Fixed stale "all (gradient mode)" display string in `MuteInputs::All` — now shows "all"
- Fixed misleading "TeamSpeak-style" comment on embedded notification sounds

## [0.2.0] - 2026-02-28

### Added

- Graceful no-device startup — tray app starts without a Scarlett device connected, shows "Disconnected" status in tray menu, and automatically connects when the device is plugged in. Hotkey, sound feedback, and notifications all work while disconnected; LED writes become no-ops until a device appears.

### Changed

- Consolidated tray menu — removed "Sound Feedback" and "Start with Windows/System" toggles (both accessible via Settings dialog) and standalone About dialog (device info moved into Settings)
- Improved settings dialog styling — grouped sections with frames, consistent button styling, section header typography, device info section
- Tuned unselected input LED white color (`0x88FFFF00` → `0xAAFFDD00`) to visually match firmware appearance on hardware

### Fixed

- Fixed deprecated `assert_cmd::Command::cargo_bin` usage in integration tests (replaced with `cargo_bin_cmd!` macro)

### Infrastructure

- Added conditional Windows code signing workflow (SignPath Foundation) — guarded by `SIGNPATH_API_TOKEN` secret in release.yml

## [0.1.0] - 2026-02-24

### Added

- Real-time mute indicator on Scarlett input number LEDs (configurable color, default red)
- System tray app with settings GUI (Windows and Linux)
- CLI interface (`focusmute-cli`) with `status`, `config`, `devices`, `monitor`, `probe`, `map`, `predict`, `descriptor`, `mute`, `unmute` subcommands and `--json` flag
- Global hotkey toggle (default: Ctrl+Shift+M)
- Sound feedback on mute/unmute (built-in or custom WAV)
- Desktop notifications on mute/unmute (optional)
- Auto-reconnect on device disconnect with exponential backoff
- Per-input targeting (all input number LEDs, or specific ones like "1" or "1,2")
- Per-input mute colors (different color per input via `input_colors` config)
- Hook commands on mute state change (`on_mute_command`, `on_unmute_command`)
- Device serial targeting for multi-device setups (`device_serial`)
- Full LED profile for Scarlett 2i2 4th Gen
- Schema-driven auto-discovery for other Scarlett 4th Gen devices
- `probe` command for device detection and schema extraction
- `map` command for interactive LED layout verification
- `predict` command for offline LED layout prediction from schema JSON
- TOML configuration file support
- Auto-launch on startup option
