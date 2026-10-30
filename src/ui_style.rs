use crate::{detector::State, win::wide};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

pub const CLIENT_WIDTH: i32 = 640;
pub const CLIENT_HEIGHT: i32 = 706;
pub const BACKGROUND: COLORREF = rgb(247, 247, 244);
pub const TEXT: COLORREF = rgb(32, 35, 33);
pub const MUTED: COLORREF = rgb(98, 103, 99);
const WHITE: COLORREF = rgb(255, 255, 255);
const BORDER: COLORREF = rgb(223, 226, 220);
const GREEN: COLORREF = rgb(35, 111, 78);
const AMBER: COLORREF = rgb(158, 79, 29);
pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

// Created only when settings are opened; all GDI objects are owned and released here.
pub struct Theme {
    pub body: HFONT,
    pub title: HFONT,
    pub section: HFONT,
    pub status: HFONT,
    pub small: HFONT,
    pub background: HBRUSH,
    pub panel: HBRUSH,
    on: HBRUSH,
    off: HBRUSH,
    unknown: HBRUSH,
}
impl Theme {
    pub unsafe fn new(scale: f64) -> Self {
        let font = |size: f64, weight: i32| unsafe {
            CreateFontW(
                -(size * scale).round() as i32,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                wide("Microsoft YaHei UI").as_ptr(),
            )
        };
        unsafe {
            Self {
                body: font(14.0, 400),
                title: font(24.0, 600),
                section: font(14.0, 600),
                status: font(18.0, 600),
                small: font(12.0, 400),
                background: CreateSolidBrush(BACKGROUND),
                panel: CreateSolidBrush(WHITE),
                on: CreateSolidBrush(rgb(237, 246, 239)),
                off: CreateSolidBrush(rgb(255, 245, 233)),
                unknown: CreateSolidBrush(rgb(239, 241, 237)),
            }
        }
    }
    pub fn status_color(state: State, paused: bool) -> COLORREF {
        if paused {
            MUTED
        } else {
            match state {
                State::On => GREEN,
                State::Off => AMBER,
                State::Unknown => MUTED,
            }
        }
    }
    pub fn status_brush(&self, state: State, paused: bool) -> HBRUSH {
        if paused {
            self.unknown
        } else {
            match state {
                State::On => self.on,
                State::Off => self.off,
                State::Unknown => self.unknown,
            }
        }
    }
    pub unsafe fn paint(&self, hwnd: HWND, scale: f64, state: State, paused: bool) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut client);
            FillRect(dc, &client, self.background);
            let n = |v: i32| (v as f64 * scale).round() as i32;
            let border = CreatePen(PS_SOLID, 1, BORDER);
            let old_pen = SelectObject(dc, border);
            let old_brush = SelectObject(dc, self.panel);
            for (top, bottom) in [(172, 290), (302, 418), (430, 530)] {
                RoundRect(dc, n(28), n(top), n(612), n(bottom), n(12), n(12));
            }
            SelectObject(dc, self.status_brush(state, paused));
            RoundRect(dc, n(28), n(80), n(612), n(156), n(12), n(12));
            let dot = CreateSolidBrush(Self::status_color(state, paused));
            SelectObject(dc, dot);
            SelectObject(dc, GetStockObject(NULL_PEN));
            Ellipse(dc, n(47), n(107), n(57), n(117));
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            DeleteObject(dot);
            DeleteObject(border);
            EndPaint(hwnd, &ps);
        }
    }
    pub unsafe fn assign_control(&self, child: HWND, id: u16, y: i32, state: State, paused: bool) {
        let status = (80..156).contains(&y);
        let brush = if status {
            self.status_brush(state, paused)
        } else if (172..530).contains(&y) || id == 101 {
            self.panel
        } else {
            self.background
        };
        let color = if id == 100 {
            Self::status_color(state, paused)
        } else if matches!(id, 118 | 101 | 119) {
            MUTED
        } else {
            TEXT
        };
        unsafe {
            SetPropW(child, wide("RG.Surface").as_ptr(), brush);
            SetPropW(child, wide("RG.TextColor").as_ptr(), color as usize as _);
        }
    }
}
impl Drop for Theme {
    fn drop(&mut self) {
        unsafe {
            for font in [self.body, self.title, self.section, self.status, self.small] {
                if !font.is_null() {
                    DeleteObject(font);
                }
            }
            for brush in [self.background, self.panel, self.on, self.off, self.unknown] {
                DeleteObject(brush);
            }
        }
    }
}

// Read primitive paint properties before borrowing application state. Windows can
// request these colors synchronously from SetWindowText or themed child painting.
pub unsafe fn control_color(child: HWND, dc: HDC) -> Option<LRESULT> {
    unsafe {
        let brush = GetPropW(child, wide("RG.Surface").as_ptr());
        if brush.is_null() {
            return None;
        }
        SetTextColor(
            dc,
            GetPropW(child, wide("RG.TextColor").as_ptr()) as usize as u32,
        );
        SetBkMode(dc, TRANSPARENT as i32);
        Some(brush as isize)
    }
}
