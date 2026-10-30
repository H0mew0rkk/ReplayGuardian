use crate::win::wide;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Auto,
    Mcp,
    Poll,
    Event,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Mcp => "NVIDIA MCP",
            Self::Poll => "注册表轮询",
            Self::Event => "注册表事件",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub interval: u64,
    pub retry: u64,
    pub restore: bool,
    pub startup: bool,
    pub notify: bool,
    pub alt: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub key: u16,
    pub registry_version: Option<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Auto,
            interval: 10,
            retry: 30,
            restore: true,
            startup: false,
            notify: false,
            alt: true,
            shift: true,
            ctrl: false,
            key: 0x79,
            registry_version: None,
        }
    }
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if ![5, 10, 30, 60].contains(&self.interval) || ![30, 60, 120, 300].contains(&self.retry) {
            return Err("检测或重试间隔无效".into());
        }
        if !key_options().iter().any(|(key, _)| *key == self.key)
            || !(self.alt || self.ctrl || self.shift)
        {
            return Err("快捷键需要至少一个修饰键及字母、数字或 F1–F24".into());
        }
        Ok(())
    }
    pub fn load() -> (Self, Option<String>) {
        match fs::read(directory().join("config.json")) {
            Ok(data) => match serde_json::from_slice::<Self>(&data)
                .map_err(|e| e.to_string())
                .and_then(|c| {
                    c.validate()?;
                    Ok(c)
                }) {
                Ok(c) => (c, None),
                Err(e) => {
                    let c = Self {
                        restore: false,
                        ..Self::default()
                    };
                    (c, Some(format!("配置无效，已关闭自动恢复：{e}")))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self {
                    restore: false,
                    ..Self::default()
                },
                Some(format!("无法读取配置：{e}")),
            ),
        }
    }
    pub fn save(&self) -> Result<(), String> {
        self.validate()?;
        let dir = directory();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let temp = dir.join("config.tmp");
        fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        unsafe {
            if windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                wide(&temp.to_string_lossy()).as_ptr(),
                wide(&dir.join("config.json").to_string_lossy()).as_ptr(),
                3,
            ) == 0
            {
                return Err(crate::win::last_error("保存配置"));
            }
        }
        Ok(())
    }
}
pub fn key_options() -> Vec<(u16, String)> {
    (0x70..=0x87)
        .map(|key| (key, format!("F{}", key - 0x70 + 1)))
        .chain(
            (b'A'..=b'Z')
                .chain(b'0'..=b'9')
                .map(|key| (key as u16, (key as char).to_string())),
        )
        .collect()
}
pub fn directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ReplayGuardian")
}
pub fn log_error(message: &str) {
    let dir = directory();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("errors.log");
    if fs::metadata(&path)
        .map(|m| m.len() > 65536)
        .unwrap_or(false)
        && (fs::copy(&path, dir.join("errors.previous.log")).is_err()
            || fs::write(&path, []).is_err())
    {
        return;
    }
    use std::io::Write;
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let bounded: String = message.chars().take(2048).collect();
        let _ = writeln!(f, "{:?}: {}", std::time::SystemTime::now(), bounded);
    }
}
pub fn set_startup(enable: bool) -> Result<(), String> {
    unsafe {
        let mut key = std::ptr::null_mut();
        let code = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if code != ERROR_SUCCESS {
            return Err(format!("自启设置失败：{code}"));
        }
        let name = wide("ReplayGuardian");
        let result = if enable {
            let exe = std::env::current_exe().map_err(|e| e.to_string());
            match exe {
                Ok(exe) => {
                    let value = wide(&format!("\"{}\" --background", exe.display()));
                    RegSetValueExW(
                        key,
                        name.as_ptr(),
                        0,
                        REG_SZ,
                        value.as_ptr().cast(),
                        (value.len() * 2) as u32,
                    )
                }
                Err(e) => {
                    RegCloseKey(key);
                    return Err(e);
                }
            }
        } else {
            RegDeleteValueW(key, name.as_ptr())
        };
        RegCloseKey(key);
        if result != ERROR_SUCCESS && !(result == ERROR_FILE_NOT_FOUND && !enable) {
            Err(format!("自启设置失败：{result}"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_safe_notifications_and_startup() {
        let c = Config::default();
        assert!(!c.notify && !c.startup);
        assert_eq!(c.interval, 10);
        c.validate().unwrap();
    }
    #[test]
    fn rejects_invalid_hotkey_and_intervals() {
        let mut c = Config {
            interval: 0,
            ..Config::default()
        };
        assert!(c.validate().is_err());
        c.interval = 10;
        c.key = 0;
        assert!(c.validate().is_err());
    }
    #[test]
    fn config_roundtrip() {
        let c = Config::default();
        let value = serde_json::to_vec(&c).unwrap();
        let restored: Config = serde_json::from_slice(&value).unwrap();
        assert_eq!(restored.mode, c.mode);
    }
}
