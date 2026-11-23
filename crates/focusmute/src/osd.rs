//! On-screen mute indicator: a small click-through banner shown for
//! [`SHOW_FOR_MS`] when the mute state changes or the user talks while muted,
//! on the monitor showing the foreground window.
//!
//! Windows only, drawn with plain GDI on a layered popup window. Nothing is
//! created until the first [`Osd::show`], and [`Osd::destroy`] releases the
//! window, so a disabled OSD costs no window, timer or thread. Elsewhere the
//! type is an inert stub.

/// What the banner announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    Muted,
    Live,
    /// Input level above the talk threshold while muted.
    Talking,
}

/// How long the banner stays visible after the last `show`.
#[cfg_attr(not(windows), allow(dead_code))]
const SHOW_FOR_MS: u32 = 1500;

#[cfg(windows)]
pub use win::Osd;

#[cfg(not(windows))]
pub struct Osd;

#[cfg(not(windows))]
impl Osd {
    pub fn new() -> Self {
        Osd
    }
    pub fn show(&mut self, _kind: OsdKind, _language: &str, _position: &str) {}
    pub fn destroy(&mut self) {}
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;

    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{PCWSTR, w};

    use super::{OsdKind, SHOW_FOR_MS};

    const CLASS_NAME: PCWSTR = w!("FocusMuteOsd");
    const HIDE_TIMER: usize = 1;
    const HEIGHT: i32 = 44;
    const PADDING_X: i32 = 28;
    const MARGIN: i32 = 24;
    const FONT_HEIGHT: i32 = 20;
    const CORNER: i32 = 16;
    const OPACITY: u8 = 230;

    /// Scale a 96-DPI length to `dpi`. A DPI-unaware process is told 96, so
    /// the banner looks the same whether or not eframe has already made the
    /// process DPI-aware by opening the settings window.
    fn px(value: i32, dpi: i32) -> i32 {
        value * dpi.max(96) / 96
    }

    /// Where the banner goes: the work area and DPI of the monitor showing the
    /// foreground window, so a full-screen game on a second monitor sees it.
    #[derive(Clone, Copy, PartialEq)]
    struct Placement {
        area: RECT,
        dpi: i32,
    }

    fn placement() -> Placement {
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        unsafe {
            let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let area = if GetMonitorInfoW(monitor, &mut info).as_bool() {
                info.rcWork
            } else {
                RECT {
                    left: 0,
                    top: 0,
                    right: GetSystemMetrics(SM_CXSCREEN),
                    bottom: GetSystemMetrics(SM_CYSCREEN),
                }
            };
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
                dpi_x = windows::Win32::UI::HiDpi::GetDpiForSystem();
            }
            Placement {
                area,
                dpi: dpi_x as i32,
            }
        }
    }

    /// Text, colour and DPI for the next `WM_PAINT`. The window lives on the
    /// main thread only, so thread-local storage is enough.
    struct PaintState {
        text: Vec<u16>,
        background: COLORREF,
        dpi: i32,
    }

    thread_local! {
        static PAINT: RefCell<PaintState> = const {
            RefCell::new(PaintState { text: Vec::new(), background: COLORREF(0), dpi: 96 })
        };
        static CLASS_REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// GDI colours are 0x00BBGGRR.
    const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
        COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
    }

    fn style(kind: OsdKind, language: &str) -> (&'static str, COLORREF) {
        let key = match kind {
            OsdKind::Muted => "osd_muted",
            OsdKind::Live => "osd_live",
            OsdKind::Talking => "osd_talking",
        };
        let color = match kind {
            OsdKind::Muted => rgb(198, 40, 40),
            OsdKind::Live => rgb(46, 125, 50),
            OsdKind::Talking => rgb(230, 100, 0),
        };
        (crate::i18n::tr(language, key), color)
    }

    fn create_font(dpi: i32) -> HFONT {
        unsafe {
            CreateFontW(
                px(FONT_HEIGHT, dpi),
                0,
                0,
                0,
                FW_SEMIBOLD.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Segoe UI"),
            )
        }
    }

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
                let mut rect = RECT::default();
                let _ = unsafe { GetClientRect(hwnd, &mut rect) };
                PAINT.with_borrow_mut(|state| unsafe {
                    let brush = CreateSolidBrush(state.background);
                    FillRect(hdc, &rect, brush);
                    let _ = DeleteObject(brush.into());
                    let font = create_font(state.dpi);
                    let old = SelectObject(hdc, font.into());
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, rgb(255, 255, 255));
                    DrawTextW(
                        hdc,
                        &mut state.text,
                        &mut rect,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    SelectObject(hdc, old);
                    let _ = DeleteObject(font.into());
                });
                let _ = unsafe { EndPaint(hwnd, &ps) };
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == HIDE_TIMER => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), HIDE_TIMER);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn ensure_class() -> bool {
        if CLASS_REGISTERED.get() {
            return true;
        }
        let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
            return false;
        };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            lpszClassName: CLASS_NAME,
            hCursor: HCURSOR::default(),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            log::warn!("[osd] could not register window class");
            return false;
        }
        CLASS_REGISTERED.set(true);
        true
    }

    /// Width of `text` in the OSD font at `dpi`, in pixels.
    fn text_width(text: &[u16], dpi: i32) -> i32 {
        unsafe {
            let hdc = GetDC(None);
            let font = create_font(dpi);
            let old = SelectObject(hdc, font.into());
            let mut size = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, text, &mut size);
            SelectObject(hdc, old);
            let _ = DeleteObject(font.into());
            ReleaseDC(None, hdc);
            size.cx
        }
    }

    /// Top-left corner for a `width`×[`HEIGHT`] banner in `place`'s work area.
    fn origin(position: &str, width: i32, place: Placement) -> (i32, i32) {
        let area = place.area;
        let (margin, height) = (px(MARGIN, place.dpi), px(HEIGHT, place.dpi));
        let left = area.left + margin;
        let right = area.right - margin - width;
        let top = area.top + margin;
        let bottom = area.bottom - margin - height;
        let center = area.left + (area.right - area.left - width) / 2;
        match position {
            "bottom_left" => (left, bottom),
            "bottom_right" => (right, bottom),
            "top_left" => (left, top),
            "top_right" => (right, top),
            _ => (center, bottom),
        }
    }

    pub struct Osd {
        hwnd: Option<HWND>,
        /// What the visible banner shows and where, to skip a repaint on a
        /// repeat. A new monitor, position or language lays it out again.
        shown: Option<(OsdKind, Placement, String)>,
    }

    impl Osd {
        pub fn new() -> Self {
            Self {
                hwnd: None,
                shown: None,
            }
        }

        fn window(&mut self) -> Option<HWND> {
            if let Some(hwnd) = self.hwnd {
                return Some(hwnd);
            }
            if !ensure_class() {
                return None;
            }
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TOPMOST
                        | WS_EX_TOOLWINDOW
                        | WS_EX_NOACTIVATE
                        | WS_EX_TRANSPARENT,
                    CLASS_NAME,
                    w!("FocusMute"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    GetModuleHandleW(None).ok().map(|m| m.into()),
                    None,
                )
            };
            match hwnd {
                Ok(hwnd) => {
                    let _ = unsafe {
                        SetLayeredWindowAttributes(hwnd, COLORREF(0), OPACITY, LWA_ALPHA)
                    };
                    self.hwnd = Some(hwnd);
                    Some(hwnd)
                }
                Err(e) => {
                    log::warn!("[osd] could not create window: {e}");
                    None
                }
            }
        }

        /// Show `kind` for [`SHOW_FOR_MS`], restarting the timer if visible.
        pub fn show(&mut self, kind: OsdKind, language: &str, position: &str) {
            let Some(hwnd) = self.window() else {
                return;
            };
            let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
            let place = placement();
            let key = (kind, place, format!("{language}/{position}"));
            if !(visible && self.shown.as_ref() == Some(&key)) {
                let dpi = place.dpi;
                let (text, background) = style(kind, language);
                let text: Vec<u16> = text.encode_utf16().collect();
                let width = text_width(&text, dpi) + px(PADDING_X, dpi) * 2;
                let height = px(HEIGHT, dpi);
                let corner = px(CORNER, dpi);
                let (x, y) = origin(position, width, place);
                PAINT.with_borrow_mut(|state| {
                    state.text = text;
                    state.background = background;
                    state.dpi = dpi;
                });
                unsafe {
                    // The region is owned by the window after SetWindowRgn.
                    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, corner, corner);
                    SetWindowRgn(hwnd, Some(region), false);
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
                self.shown = Some(key);
            }
            // Re-arming an existing timer id replaces its interval.
            unsafe { SetTimer(Some(hwnd), HIDE_TIMER, SHOW_FOR_MS, None) };
        }

        /// Release the window. The next `show` recreates it.
        pub fn destroy(&mut self) {
            if let Some(hwnd) = self.hwnd.take() {
                let _ = unsafe { DestroyWindow(hwnd) };
            }
            self.shown = None;
        }
    }

    impl Drop for Osd {
        fn drop(&mut self) {
            self.destroy();
        }
    }
}
