use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub struct Handle(pub HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
pub fn last_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}
