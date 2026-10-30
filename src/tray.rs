use crate::win::last_error;
use windows_sys::Win32::UI::{Shell::*, WindowsAndMessaging::*};

pub struct Icon(pub HICON);
impl Icon {
    pub fn new() -> Result<Self, String> {
        // A green R remains recognizable when Windows places it in tray overflow.
        let mut mask = [255u8; 128];
        let mut pixels = [0u8; 32 * 32 * 4];
        let letter = [
            0b11110u8, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ];
        for y in 0..32usize {
            for x in 0..32usize {
                let corner_x = if x < 6 { 6 - x } else { x.saturating_sub(25) };
                let corner_y = if y < 6 { 6 - y } else { y.saturating_sub(25) };
                if corner_x * corner_x + corner_y * corner_y > 36 {
                    continue;
                }
                mask[y * 4 + x / 8] &= !(0x80 >> (x % 8));
                let white = (8..23).contains(&x)
                    && (5..26).contains(&y)
                    && letter[(y - 5) / 3] & (1 << (4 - (x - 8) / 3)) != 0;
                let pixel = &mut pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4];
                pixel.copy_from_slice(if white {
                    &[255, 255, 255, 255]
                } else {
                    &[78, 111, 35, 255]
                });
            }
        }
        let icon = unsafe {
            CreateIcon(
                std::ptr::null_mut(),
                32,
                32,
                1,
                32,
                mask.as_ptr(),
                pixels.as_ptr(),
            )
        };
        if icon.is_null() {
            Err(last_error("创建托盘图标"))
        } else {
            Ok(Self(icon))
        }
    }
}
impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

// Recover both an Explorer-lost icon and an already-existing icon. State always
// reflects this attempt, never a success remembered from earlier in the session.
pub fn ensure_icon(known: bool, force_add: bool, mut notify: impl FnMut(u32) -> bool) -> bool {
    let first = if known && !force_add {
        NIM_MODIFY
    } else {
        NIM_ADD
    };
    notify(first)
        || notify(if first == NIM_ADD {
            NIM_MODIFY
        } else {
            NIM_ADD
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_icon_is_added_again() {
        let mut calls = Vec::new();
        assert!(ensure_icon(true, false, |op| {
            calls.push(op);
            op == NIM_ADD
        }));
        assert_eq!(calls, [NIM_MODIFY, NIM_ADD]);
    }
    #[test]
    fn explorer_recovery_handles_existing_icon() {
        let mut calls = Vec::new();
        assert!(ensure_icon(false, true, |op| {
            calls.push(op);
            op == NIM_MODIFY
        }));
        assert_eq!(calls, [NIM_ADD, NIM_MODIFY]);
    }
    #[test]
    fn both_shell_failures_report_unavailable() {
        assert!(!ensure_icon(true, false, |_| false));
    }
}
