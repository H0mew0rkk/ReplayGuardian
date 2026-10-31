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
            interval: 60,
            retry: 120,
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
    static LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = directory();
    let _ = fs::create_dir_all(&dir);
    let bounded: String = message.chars().take(2048).collect();
    let entry = format!("{:?}: {}\n", std::time::SystemTime::now(), bounded);
    let _ = append_log(&dir.join("errors.log"), entry.as_bytes());
}

const MAX_LOG_BYTES: u64 = 64 * 1024;
const RETAIN_LOG_BYTES: usize = 32 * 1024;

fn append_log(path: &std::path::Path, entry: &[u8]) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    if len.saturating_add(entry.len() as u64) > MAX_LOG_BYTES {
        let keep = len.min(RETAIN_LOG_BYTES.saturating_sub(entry.len()) as u64);
        file.seek(SeekFrom::End(-(keep as i64)))?;
        let mut tail = Vec::with_capacity(keep as usize);
        file.read_to_end(&mut tail)?;
        // The tail may start inside a UTF-8 character or a record. Skip that record.
        let start = tail
            .iter()
            .position(|b| *b == b'\n')
            .map_or(tail.len(), |i| i + 1);
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&tail[start..])?;
        file.set_len((tail.len() - start) as u64)?;
    }
    file.write_all(entry)
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
        assert_eq!(c.interval, 60);
        assert_eq!(c.retry, 120);
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

    #[test]
    fn log_keeps_recent_complete_utf8_lines_within_size_limit() {
        let path = std::env::temp_dir().join(format!(
            "replay-guardian-log-test-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Filling the file exactly to the limit must preserve all existing data.
        let original = format!("{}\n", "x".repeat(MAX_LOG_BYTES as usize - 3));
        fs::write(&path, &original).unwrap();
        append_log(&path, b"a\n").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_LOG_BYTES);
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{original}a\n"));

        // Crossing the limit drops the oldest line but keeps the latest lines.
        append_log(&path, "最新错误\n".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\n最新错误\n");

        // An oversized legacy file is compacted without splitting Chinese text.
        let old = "历史错误：自动恢复失败\n";
        fs::write(&path, old.repeat(10000)).unwrap();
        append_log(&path, "本次错误\n".as_bytes()).unwrap();
        let recent = fs::read_to_string(&path).unwrap();
        assert!(recent.len() <= RETAIN_LOG_BYTES);
        assert!(recent.ends_with("本次错误\n"));
        assert!(recent.lines().count() > 1);
        assert!(
            recent
                .lines()
                .all(|line| line == old.trim_end() || line == "本次错误")
        );
        append_log(&path, "继续写入\n".as_bytes()).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{recent}继续写入\n")
        );
        fs::remove_file(path).unwrap();
    }
}
