use crate::{config::Config, detector::State};
use windows_sys::Win32::{System::StationsAndDesktops::*, UI::Input::KeyboardAndMouse::*};
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    None,
    Restore,
    Recovered,
}
#[derive(Default)]
pub struct Guard {
    off_since: Option<u64>,
    pending: Option<u64>,
    next_attempt: u64,
    failures: u32,
}
impl Guard {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn cancel(&mut self, now: u64) {
        if self.pending.take().is_some() {
            self.next_attempt = self.next_attempt.max(now + 30);
        }
        self.off_since = None;
    }
    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn failures(&self) -> u32 {
        self.failures
    }
    pub fn observe(&mut self, now: u64, state: State, enabled: bool, retry: u64) -> Decision {
        if !enabled {
            self.cancel(now);
            return Decision::None;
        }
        if let Some(start) = self.pending {
            if state == State::On && now <= start + 15 {
                self.reset();
                return Decision::Recovered;
            }
            if now < start + 15 {
                return Decision::None;
            }
            self.pending = None;
            self.off_since = None;
            self.failures = self.failures.saturating_add(1);
            let delay = retry
                .max(30)
                .saturating_mul(1u64 << self.failures.saturating_sub(1).min(4))
                .min(300);
            self.next_attempt = now + if state == State::Unknown { 300 } else { delay };
        }
        match state {
            State::On => {
                self.reset();
            }
            State::Unknown => {
                self.off_since = None;
            }
            State::Off => {
                let first = *self.off_since.get_or_insert(now);
                if now >= self.next_attempt && now >= first + 2 {
                    self.off_since = None;
                    return Decision::Restore;
                }
            }
        }
        Decision::None
    }
    pub fn attempted(&mut self, now: u64) {
        self.pending = Some(now);
        self.off_since = None;
    }
    pub fn rejected(&mut self, now: u64) {
        self.off_since = None;
        self.next_attempt = now + 300;
    }
}
pub fn send_hotkey(c: &Config) -> Result<(), String> {
    unsafe {
        // Refuse to inject on the secure/locked desktop or into a user's held chord.
        let desktop = OpenInputDesktop(0, 0, 0x0001);
        if desktop.is_null() {
            return Err("当前输入桌面不可用".into());
        }
        CloseDesktop(desktop);
        for key in [VK_MENU, VK_SHIFT, VK_CONTROL, VK_LWIN, VK_RWIN, c.key] {
            if GetAsyncKeyState(key as i32) < 0 {
                return Err("用户正在按键，暂缓恢复".into());
            }
        }
        let mut keys = Vec::new();
        if c.ctrl {
            keys.push(VK_CONTROL);
        }
        if c.alt {
            keys.push(VK_MENU);
        }
        if c.shift {
            keys.push(VK_SHIFT);
        }
        keys.push(c.key);
        let mut inputs = Vec::new();
        for key in &keys {
            inputs.push(input(*key, 0));
        }
        for key in keys.iter().rev() {
            inputs.push(input(*key, KEYEVENTF_KEYUP));
        }
        let sent = SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
        if sent != inputs.len() as u32 {
            let releases: Vec<_> = keys
                .iter()
                .rev()
                .map(|k| input(*k, KEYEVENTF_KEYUP))
                .collect();
            SendInput(
                releases.len() as u32,
                releases.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
            return Err("SendInput 未完整送达，结果未知；不会立即补发".into());
        }
        Ok(())
    }
}
fn input(key: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_or_pause_cannot_erase_inflight_cooldown() {
        let mut g = Guard::default();
        g.attempted(0);
        g.cancel(1);
        assert_eq!(g.observe(2, State::Off, true, 30), Decision::None);
        assert_eq!(g.observe(4, State::Off, true, 30), Decision::None);
        g.cancel(5);
        assert_eq!(g.observe(10, State::Off, true, 30), Decision::None);
        assert_eq!(g.observe(31, State::Off, true, 30), Decision::Restore);
    }
    #[test]
    fn unknown_never_restores() {
        let mut g = Guard::default();
        for n in 0..100 {
            assert_eq!(g.observe(n, State::Unknown, true, 30), Decision::None);
        }
    }
    #[test]
    fn requires_two_off_samples() {
        let mut g = Guard::default();
        assert_eq!(g.observe(0, State::Off, true, 30), Decision::None);
        assert_eq!(g.observe(2, State::Off, true, 30), Decision::Restore);
    }
    #[test]
    fn on_or_unknown_breaks_off_streak() {
        for state in [State::On, State::Unknown] {
            let mut g = Guard::default();
            g.observe(0, State::Off, true, 30);
            g.observe(2, state, true, 30);
            assert_eq!(g.observe(3, State::Off, true, 30), Decision::None);
        }
    }
    #[test]
    fn pending_prevents_duplicate_input() {
        let mut g = Guard::default();
        g.attempted(0);
        for n in 1..15 {
            assert_eq!(g.observe(n, State::Off, true, 30), Decision::None);
        }
    }
    #[test]
    fn notification_only_in_confirmation_window() {
        let mut g = Guard::default();
        assert_eq!(g.observe(0, State::On, true, 30), Decision::None);
        g.attempted(1);
        assert_eq!(g.observe(3, State::On, true, 30), Decision::Recovered);
        assert_eq!(g.observe(4, State::On, true, 30), Decision::None);
    }
    #[test]
    fn late_on_is_not_recovered() {
        let mut g = Guard::default();
        g.attempted(0);
        assert_eq!(g.observe(20, State::On, true, 30), Decision::None);
    }
    #[test]
    fn failure_backs_off() {
        let mut g = Guard::default();
        g.attempted(0);
        g.observe(15, State::Off, true, 30);
        assert_eq!(g.observe(20, State::Off, true, 30), Decision::None);
        assert_eq!(g.observe(45, State::Off, true, 30), Decision::Restore);
    }
    #[test]
    fn pause_cancels_pending_success() {
        let mut g = Guard::default();
        g.attempted(0);
        assert_eq!(g.observe(2, State::On, false, 30), Decision::None);
    }
}
