#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
mod config;
mod detector;
mod monitor;
mod recovery;
mod tray;
mod ui;
mod ui_style;
mod win;

fn main() {
    if let Err(error) = ui::run() {
        config::log_error(&error);
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                win::wide(&error).as_ptr(),
                win::wide("Replay Guardian").as_ptr(),
                0x10,
            );
        }
    }
}
