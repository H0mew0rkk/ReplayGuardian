use crate::{
    config::{Config, Mode, log_error},
    detector::{
        State,
        mcp::Mcp,
        registry::{self, Watch},
    },
    recovery::{Decision, Guard},
    win::{Handle, last_error},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*, System::Threading::*, UI::WindowsAndMessaging::PostMessageW,
};
pub const UPDATE: u32 = 0x8001;
pub enum Command {
    Configure(Config, bool),
    Test,
    Calibrate,
    CancelCalibration,
    Exit,
}
#[derive(Clone)]
pub struct Snapshot {
    pub state: State,
    pub actual: Mode,
    pub detail: String,
    pub recovered: bool,
    pub calibration: u8,
    pub calibrated_version: Option<String>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: State::Unknown,
            actual: Mode::Mcp,
            detail: "正在连接…".into(),
            recovered: false,
            calibration: 0,
            calibrated_version: None,
        }
    }
}
pub struct Worker {
    tx: mpsc::Sender<Command>,
    pub rx: mpsc::Receiver<Snapshot>,
    event: Arc<Handle>,
    epoch: Arc<AtomicU64>,
    join: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(hwnd: usize, config: Config) -> Result<Self, String> {
        let event = Arc::new(Handle(unsafe {
            CreateEventW(std::ptr::null(), 0, 0, std::ptr::null())
        }));
        if event.0.is_null() {
            return Err(last_error("创建控制事件"));
        }
        let (tx, rx) = mpsc::channel();
        let (out, input) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        let wake = event.clone();
        let revision = epoch.clone();
        let join = thread::Builder::new()
            .name("replay-monitor".into())
            .spawn(move || run(hwnd, config, rx, out, wake, revision))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx,
            rx: input,
            event,
            epoch,
            join: Some(join),
        })
    }
    pub fn send(&self, command: Command) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(command);
        unsafe {
            SetEvent(self.event.0);
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.send(Command::Exit);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn publish(hwnd: usize, tx: &mpsc::Sender<Snapshot>, value: &Snapshot) {
    let _ = tx.send(value.clone());
    unsafe {
        PostMessageW(hwnd as _, UPDATE, 0, 0);
    }
}
fn trusted(config: &Config) -> bool {
    let current = registry::fingerprint();
    current.is_some() && current == config.registry_version
}
fn select_mode(
    requested: Mode,
    mcp_ready: bool,
    registry_trusted: bool,
    event_ready: bool,
) -> Mode {
    if requested != Mode::Auto {
        return requested;
    }
    if mcp_ready || !registry_trusted {
        Mode::Mcp
    } else if event_ready {
        Mode::Event
    } else {
        Mode::Poll
    }
}
fn restricted(error: &str) -> bool {
    error.contains("权限")
        || error.contains("受保护")
        || error.contains("access_level_restricted")
        || error.contains("protected")
        || error.contains("DRM")
}
fn read(mode: Mode, mcp: &mut Option<Mcp>, config: &Config) -> Result<State, String> {
    if mode == Mode::Mcp {
        mcp.as_mut()
            .ok_or_else(|| "MCP 不可用，等待重连".to_string())?
            .status()
    } else if !trusted(config) {
        Err("注册表尚未通过本机 开→关→开 校验，自动恢复不可用".into())
    } else if !registry::overlay_running() {
        Err("NVIDIA Overlay 未运行，注册表可能是残留状态".into())
    } else {
        registry::read()
    }
}
fn run(
    hwnd: usize,
    mut config: Config,
    commands: mpsc::Receiver<Command>,
    out: mpsc::Sender<Snapshot>,
    event: Arc<Handle>,
    epoch: Arc<AtomicU64>,
) {
    let clock = Instant::now();
    let mut guard = Guard::default();
    let mut mcp: Option<Mcp> = None;
    let mut watch: Option<Watch> = None;
    let mut snapshot = Snapshot::default();
    let mut paused = false;
    let mut next = Instant::now();
    let mut reconnect = Instant::now();
    let mut upgrade = Instant::now();
    let mut failures = 0u32;
    let mut last_error_text = String::new();
    let mut calibration = 0u8;
    let mut recovery_restricted = false;
    let mut recovery_message = String::new();
    loop {
        let mut force = false;
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Exit => return,
                Command::Configure(c, p) => {
                    let changed = c.mode != config.mode;
                    config = c;
                    paused = p;
                    guard.cancel(clock.elapsed().as_secs());
                    if changed {
                        watch = None;
                        mcp = None;
                        reconnect = Instant::now();
                        failures = 0;
                    }
                    next = Instant::now();
                    force = true;
                }
                Command::CancelCalibration => {
                    calibration = 0;
                    guard.cancel(clock.elapsed().as_secs());
                    force = true;
                }
                Command::Test => {
                    guard.cancel(clock.elapsed().as_secs());
                    let mut messages = Vec::new();
                    match Mcp::connect() {
                        Ok(mut c) => messages.push(format!(
                            "MCP：{:?}；明确开启工具：{}",
                            c.status(),
                            c.can_enable
                        )),
                        Err(e) => messages.push(format!("MCP：{e}")),
                    };
                    messages.push(format!(
                        "注册表原始读取：{:?}；已校验：{}",
                        registry::read(),
                        trusted(&config)
                    ));
                    messages.push(format!(
                        "事件监听：{}",
                        if Watch::new().is_ok() {
                            "可订阅"
                        } else {
                            "不可用"
                        }
                    ));
                    snapshot.detail = messages.join("\r\n");
                    snapshot.recovered = false;
                    publish(hwnd, &out, &snapshot);
                    next = Instant::now() + Duration::from_secs(config.interval);
                    continue;
                }
                Command::Calibrate => {
                    guard.cancel(clock.elapsed().as_secs());
                    let expected = if calibration == 2 {
                        State::Off
                    } else {
                        State::On
                    };
                    if calibration == 0 {
                        calibration = 1;
                    }
                    let raw = registry::read();
                    let agreement = mcp
                        .as_mut()
                        .and_then(|c| c.status().ok())
                        .map(|s| s == expected)
                        .unwrap_or(true);
                    if raw == Ok(expected) && agreement && registry::overlay_running() {
                        calibration += 1;
                        if calibration == 4 {
                            if let Some(version) = registry::fingerprint() {
                                config.registry_version = Some(version.clone());
                                snapshot.calibrated_version = Some(version);
                                if let Err(e) = config.save() {
                                    snapshot.detail = e;
                                } else {
                                    snapshot.detail="注册表 开→关→开 校验完成；仍需验证受保护内容和进程重启场景".into();
                                }
                                calibration = 0;
                            } else {
                                snapshot.detail = "无法识别 NVIDIA 安装版本，不能保存校验".into();
                                calibration = 1;
                            }
                        } else {
                            snapshot.detail = if calibration == 2 {
                                "已记录开启。请在 NVIDIA 中手动关闭，再点击校验。"
                            } else {
                                "已记录关闭。请在 NVIDIA 中手动开启，再点击校验。"
                            }
                            .into();
                        }
                    } else {
                        snapshot.detail = format!(
                            "请先在 NVIDIA 中设置为 {}；候选值读取：{raw:?}",
                            expected.label()
                        );
                    }
                    snapshot.calibration = calibration;
                    snapshot.recovered = false;
                    publish(hwnd, &out, &snapshot);
                    next = Instant::now() + Duration::from_secs(60);
                    continue;
                }
            }
        }
        if paused || calibration != 0 {
            snapshot.calibration = calibration;
            snapshot.recovered = false;
            if force {
                snapshot.detail = if paused {
                    "监控已暂停"
                } else {
                    "校验进行中，自动恢复已暂停"
                }
                .into();
                publish(hwnd, &out, &snapshot);
            }
            unsafe {
                WaitForSingleObject(event.0, INFINITE);
            }
            continue;
        }
        if Instant::now() < next && !force {
            let timeout = next
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX as u128 - 1) as u32;
            let mut handles = vec![event.0];
            if let Some(w) = &watch {
                handles.push(w.event.0);
            }
            let result = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout)
            };
            if result == WAIT_OBJECT_0 {
                continue;
            }
            if result == WAIT_OBJECT_0 + 1 && watch.as_ref().is_some_and(|w| w.arm().is_err()) {
                watch = None;
            }
        }
        let generation = epoch.load(Ordering::SeqCst);
        let now = clock.elapsed().as_secs();
        snapshot.recovered = false;
        let desired = if config.mode == Mode::Auto {
            if mcp.is_none() && Instant::now() >= reconnect && Instant::now() >= upgrade {
                match Mcp::connect() {
                    Ok(c) => {
                        recovery_restricted = !c.can_enable;
                        mcp = Some(c);
                        failures = 0;
                    }
                    Err(e) => {
                        recovery_restricted |= restricted(&e);
                        snapshot.detail = e;
                        reconnect = Instant::now() + Duration::from_secs(300);
                        failures = 3;
                    }
                }
            }
            if !(mcp.is_some() && failures < 3) && trusted(&config) && watch.is_none() {
                watch = Watch::new().ok();
            }
            select_mode(
                Mode::Auto,
                mcp.is_some() && failures < 3,
                trusted(&config),
                watch.is_some(),
            )
        } else {
            config.mode
        };
        if desired != snapshot.actual {
            guard.cancel(clock.elapsed().as_secs());
            snapshot.actual = desired;
        }
        if desired != Mode::Event {
            watch = None;
        }
        // Detection choice does not prevent preferring explicit MCP enable for recovery.
        if mcp.is_none() && Instant::now() >= reconnect && Instant::now() >= upgrade {
            match Mcp::connect() {
                Ok(c) => {
                    recovery_restricted = !c.can_enable;
                    mcp = Some(c);
                }
                Err(e) => {
                    recovery_restricted |= restricted(&e);
                    snapshot.detail = e;
                    reconnect = Instant::now() + Duration::from_secs(300);
                }
            }
        }
        if desired == Mode::Event && watch.is_none() {
            match Watch::new() {
                Ok(w) => watch = Some(w),
                Err(e) => {
                    snapshot.detail = e;
                }
            }
        }
        let result = if desired == Mode::Event && watch.is_none() {
            Err("事件监听不可用，等待重新订阅".into())
        } else {
            read(desired, &mut mcp, &config)
        };
        snapshot.state = match result {
            Ok(state) => {
                failures = 0;
                snapshot.detail = if guard.pending() {
                    "正在确认恢复结果…"
                } else {
                    "监控中"
                }
                .into();
                state
            }
            Err(e) => {
                recovery_restricted |= restricted(&e);
                failures += 1;
                if desired == Mode::Mcp && failures >= 3 {
                    mcp = None;
                    upgrade = Instant::now() + Duration::from_secs(300);
                    reconnect = upgrade;
                }
                if e != last_error_text {
                    log_error(&e);
                    last_error_text = e.clone();
                }
                snapshot.detail = e;
                State::Unknown
            }
        };
        if epoch.load(Ordering::SeqCst) != generation {
            continue;
        }
        let previous_failures = guard.failures();
        let decision = guard.observe(
            now,
            snapshot.state,
            config.restore && !recovery_restricted,
            config.retry,
        );
        if guard.failures() > previous_failures {
            let message = format!(
                "恢复未确认，状态 {:?}，进入退避等待。NVIDIA 返回：{}",
                snapshot.state, recovery_message
            );
            log_error(&message);
            snapshot.detail = message;
        }
        match decision {
            Decision::Recovered => {
                snapshot.recovered = true;
                snapshot.detail = "已确认即时回放开启".into();
            }
            Decision::Restore => {
                // A final read narrows, but cannot eliminate, the hotkey toggle race.
                if read(desired, &mut mcp, &config) == Ok(State::Off)
                    && epoch.load(Ordering::SeqCst) == generation
                {
                    let recovery = if let Some(c) = mcp.as_mut() {
                        if !c.can_enable {
                            Err("MCP 无明确开启权限；不使用快捷键绕过".into())
                        } else {
                            match c.status() {
                                Ok(State::Off) if epoch.load(Ordering::SeqCst) == generation => {
                                    guard.attempted(clock.elapsed().as_secs());
                                    c.enable()
                                }
                                _ => Err("官方状态不再明确为关闭，已取消恢复".into()),
                            }
                        }
                    } else if trusted(&config) && registry::overlay_running() {
                        guard.attempted(clock.elapsed().as_secs());
                        crate::recovery::send_hotkey(&config).map(|()| "备用快捷键已发送".into())
                    } else {
                        Err("没有可信的恢复方式".into())
                    };
                    if let Err(e) = &recovery {
                        recovery_restricted |= restricted(e);
                        snapshot.detail = e.clone();
                        log_error(e);
                        if !guard.pending() {
                            guard.rejected(now);
                        }
                    } else {
                        recovery_message = recovery.unwrap_or_default();
                        recovery_restricted |= restricted(&recovery_message);
                        snapshot.detail = format!("{}；等待状态确认…", recovery_message);
                    }
                } else {
                    guard.rejected(now);
                }
            }
            Decision::None => {}
        }
        snapshot.calibration = calibration;
        publish(hwnd, &out, &snapshot);
        let seconds = if guard.pending() {
            2
        } else if snapshot.state == State::Off && config.restore {
            config.interval.min(2)
        } else if desired == Mode::Event {
            60
        } else {
            config.interval
        };
        next = Instant::now() + Duration::from_secs(seconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_fallback_order() {
        assert_eq!(select_mode(Mode::Auto, true, true, true), Mode::Mcp);
        assert_eq!(select_mode(Mode::Auto, false, true, true), Mode::Event);
        assert_eq!(select_mode(Mode::Auto, false, true, false), Mode::Poll);
        assert_eq!(select_mode(Mode::Auto, false, false, true), Mode::Mcp);
    }
    #[test]
    fn explicit_modes_do_not_fall_back() {
        for mode in [Mode::Mcp, Mode::Event, Mode::Poll] {
            assert_eq!(select_mode(mode, false, false, false), mode);
        }
    }
    #[test]
    fn unavailable_backend_cannot_initiate_restore() {
        let mut g = Guard::default();
        for now in 0..100 {
            assert_eq!(g.observe(now, State::Unknown, true, 30), Decision::None);
        }
    }
    #[test]
    fn permission_errors_disable_recovery_fallback() {
        assert!(restricted("access_level_restricted"));
        assert!(restricted("MCP 权限受限 HTTP 403"));
        assert!(!restricted("连接超时"));
    }
}
