use crate::{
    detector::{State, parse_registry},
    win::{Handle, wide},
};
use windows_sys::Win32::System::{Registry::*, Threading::*};
pub const PATH: &str = "Software\\NVIDIA Corporation\\Global\\ShadowPlay\\NVSPCAPS";
const VALUE: &str = "{1B1D3DAA-601D-49E5-8508-81736CA28C6D}";
pub struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn open(access: u32) -> Result<Key, String> {
    unsafe {
        let mut key = std::ptr::null_mut();
        let code = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(PATH).as_ptr(),
            0,
            access | KEY_WOW64_64KEY,
            &mut key,
        );
        if code != 0 {
            Err(format!("无法打开 NVIDIA 注册表：{code}"))
        } else {
            Ok(Key(key))
        }
    }
}
pub fn read() -> Result<State, String> {
    let key = open(KEY_QUERY_VALUE)?;
    unsafe {
        let mut kind = 0;
        let mut bytes = [0u8; 4];
        let mut length = 4;
        let code = RegQueryValueExW(
            key.0,
            wide(VALUE).as_ptr(),
            std::ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut length,
        );
        if code != 0 || length != 4 {
            return Err(format!("读取候选注册表状态失败：{code}，长度 {length}"));
        }
        parse_registry(kind, &bytes)
    }
}
pub struct Watch {
    key: Key,
    pub event: Handle,
}
impl Watch {
    pub fn new() -> Result<Self, String> {
        let key = open(KEY_NOTIFY | KEY_QUERY_VALUE)?;
        let event = Handle(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) });
        if event.0.is_null() {
            return Err(crate::win::last_error("创建注册表事件"));
        }
        let result = Self { key, event };
        result.arm()?;
        Ok(result)
    }
    pub fn arm(&self) -> Result<(), String> {
        let code = unsafe {
            RegNotifyChangeKeyValue(
                self.key.0,
                0,
                REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC,
                self.event.0,
                1,
            )
        };
        if code == 0 {
            Ok(())
        } else {
            Err(format!("注册表监听失败：{code}"))
        }
    }
}
pub fn fingerprint() -> Option<String> {
    let base = std::env::var_os("ProgramFiles")?;
    let path = std::path::PathBuf::from(base).join("NVIDIA Corporation/NVIDIA App/NvAppApi.dll");
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!(
        "{}:{}",
        metadata.len(),
        metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    ))
}
pub fn overlay_running() -> bool {
    // Process enumeration is performed only before a registry recovery, never in a busy loop.
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::*;
        let mut ids = [0u32; 4096];
        let mut bytes = 0;
        if K32EnumProcesses(
            ids.as_mut_ptr(),
            std::mem::size_of_val(&ids) as u32,
            &mut bytes,
        ) == 0
        {
            return false;
        }
        for id in ids
            .iter()
            .take((bytes as usize / 4).min(ids.len()))
            .copied()
            .filter(|v| *v != 0)
        {
            let handle = Handle(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, id));
            if handle.0.is_null() {
                continue;
            }
            let mut path = [0u16; 1024];
            let mut len = path.len() as u32;
            if QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut len) != 0
                && String::from_utf16_lossy(&path[..len as usize])
                    .to_lowercase()
                    .ends_with("\\nvidia overlay.exe")
            {
                return true;
            }
        }
        false
    }
}
