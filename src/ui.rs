use crate::{
    config::{self, Config, Mode},
    monitor::{self, Command, Snapshot, Worker},
    win::{Handle, last_error, wide},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ptr::{null, null_mut},
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{Controls::*, Shell::*, WindowsAndMessaging::*},
};
const TRAY: u32 = WM_APP + 2;
const STATUS: u16 = 100;
const DETAIL: u16 = 101;
const MODE: u16 = 102;
const INTERVAL: u16 = 103;
const RETRY: u16 = 104;
const RESTORE: u16 = 105;
const STARTUP: u16 = 106;
const NOTIFY: u16 = 107;
const ALT: u16 = 108;
const SHIFT: u16 = 109;
const CTRL: u16 = 110;
const KEY: u16 = 111;
const TEST: u16 = 112;
const CALIBRATE: u16 = 113;
const CANCEL: u16 = 114;
const PAUSE: u16 = 115;
const EXIT: u16 = 116;
const SETTINGS: u16 = 117;
struct App {
    config: Config,
    paused: bool,
    snapshot: Snapshot,
    worker: Option<Worker>,
    controls: BTreeMap<u16, HWND>,
    taskbar: u32,
    tray_added: bool,
    scale: f64,
    safe: bool,
}
// This small wrapper mirrors CreateWindowEx's native control geometry.
#[allow(clippy::too_many_arguments)]
fn control(
    hwnd: HWND,
    class: &str,
    text: &str,
    id: u16,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    style: u32,
    scale: f64,
) -> HWND {
    unsafe {
        let c = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            (x as f64 * scale) as i32,
            (y as f64 * scale) as i32,
            (w as f64 * scale) as i32,
            (h as f64 * scale) as i32,
            hwnd,
            id as usize as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(c, WM_SETFONT, GetStockObject(DEFAULT_GUI_FONT) as usize, 1);
        c
    }
}
unsafe fn checkbox(hwnd: HWND, checked: bool) {
    SendMessageW(
        hwnd,
        BM_SETCHECK,
        if checked { BST_CHECKED } else { BST_UNCHECKED } as usize,
        0,
    );
}
unsafe fn checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}
unsafe fn selected(hwnd: HWND) -> usize {
    SendMessageW(hwnd, CB_GETCURSEL, 0, 0).max(0) as usize
}
impl App {
    unsafe fn build(&mut self, hwnd: HWND) {
        let s = self.scale;
        control(hwnd, "STATIC", "Replay Guardian", 0, 20, 16, 480, 25, 0, s);
        self.controls.insert(
            STATUS,
            control(
                hwnd,
                "STATIC",
                "状态未知 · 正在连接",
                STATUS,
                20,
                48,
                480,
                25,
                0,
                s,
            ),
        );
        self.controls.insert(
            DETAIL,
            control(hwnd, "STATIC", "", DETAIL, 20, 78, 480, 82, 0, s),
        );
        for (label, y) in [
            ("检测方式", 170),
            ("检测间隔", 208),
            ("重试基础间隔", 246),
            ("即时回放快捷键", 412),
        ] {
            control(hwnd, "STATIC", label, 0, 20, y + 4, 150, 24, 0, s);
        }
        for (id, y, entries, index) in [
            (
                MODE,
                170,
                vec!["自动选择", "NVIDIA MCP", "注册表轮询", "注册表事件监听"],
                match self.config.mode {
                    Mode::Auto => 0,
                    Mode::Mcp => 1,
                    Mode::Poll => 2,
                    Mode::Event => 3,
                },
            ),
            (
                INTERVAL,
                208,
                vec!["5 秒", "10 秒", "30 秒", "60 秒"],
                [5, 10, 30, 60]
                    .iter()
                    .position(|v| *v == self.config.interval)
                    .unwrap_or(1),
            ),
            (
                RETRY,
                246,
                vec!["30 秒", "60 秒", "120 秒", "300 秒"],
                [30, 60, 120, 300]
                    .iter()
                    .position(|v| *v == self.config.retry)
                    .unwrap_or(0),
            ),
        ] {
            let c = control(
                hwnd,
                "COMBOBOX",
                "",
                id,
                180,
                y,
                300,
                200,
                CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
                s,
            );
            for entry in entries {
                SendMessageW(c, CB_ADDSTRING, 0, wide(entry).as_ptr() as isize);
            }
            SendMessageW(c, CB_SETCURSEL, index, 0);
            self.controls.insert(id, c);
        }
        for (id, label, y, value) in [
            (RESTORE, "自动恢复即时回放", 290, self.config.restore),
            (STARTUP, "当前用户登录时启动", 324, self.config.startup),
            (
                NOTIFY,
                "确认恢复成功后显示系统通知（默认关闭）",
                358,
                self.config.notify,
            ),
        ] {
            let c = control(
                hwnd,
                "BUTTON",
                label,
                id,
                20,
                y,
                470,
                28,
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                s,
            );
            checkbox(c, value);
            self.controls.insert(id, c);
        }
        for (id, label, x, value) in [
            (ALT, "Alt", 180, self.config.alt),
            (SHIFT, "Shift", 250, self.config.shift),
            (CTRL, "Ctrl", 330, self.config.ctrl),
        ] {
            let c = control(
                hwnd,
                "BUTTON",
                label,
                id,
                x,
                412,
                75,
                28,
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                s,
            );
            checkbox(c, value);
            self.controls.insert(id, c);
        }
        let c = control(
            hwnd,
            "COMBOBOX",
            "",
            KEY,
            410,
            412,
            70,
            200,
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
            s,
        );
        let keys = config::key_options();
        for (_, label) in &keys {
            SendMessageW(c, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
        }
        SendMessageW(
            c,
            CB_SETCURSEL,
            keys.iter()
                .position(|(key, _)| *key == self.config.key)
                .unwrap_or(9),
            0,
        );
        self.controls.insert(KEY, c);
        control(
            hwnd,
            "STATIC",
            "保持回放关闭时请先暂停监控。快捷键须与 NVIDIA 设置一致。",
            0,
            20,
            455,
            480,
            36,
            0,
            s,
        );
        for (id, label, x, w) in [
            (TEST, "测试检测方式", 20, 145),
            (CALIBRATE, "校验注册表…", 180, 155),
            (CANCEL, "取消校验", 350, 130),
        ] {
            let c = control(hwnd, "BUTTON", label, id, x, 500, w, 30, WS_TABSTOP, s);
            self.controls.insert(id, c);
        }
        control(
            hwnd,
            "STATIC",
            "设置修改后自动保存；关闭此窗口后仍在托盘运行。",
            0,
            20,
            548,
            480,
            26,
            0,
            s,
        );
        if self.safe {
            EnableWindow(self.controls[&RESTORE], 0);
            EnableWindow(self.controls[&STARTUP], 0);
            EnableWindow(self.controls[&CALIBRATE], 0);
            EnableWindow(self.controls[&CANCEL], 0);
        }
    }
    unsafe fn update(&mut self, hwnd: HWND) {
        if let Some(worker) = &self.worker {
            while let Ok(snapshot) = worker.rx.try_recv() {
                if let Some(version) = &snapshot.calibrated_version {
                    self.config.registry_version = Some(version.clone());
                }
                if snapshot.recovered && self.config.notify && !self.paused && !self.safe {
                    self.balloon(hwnd);
                }
                self.snapshot = snapshot;
            }
        }
        let text = format!(
            "{} · {}{}",
            self.snapshot.state.label(),
            self.snapshot.actual.label(),
            if self.paused { " · 已暂停" } else { "" }
        );
        if self.controls.is_empty() {
            self.tray(hwnd, false);
            return;
        }
        SetWindowTextW(self.controls[&STATUS], wide(&text).as_ptr());
        SetWindowTextW(self.controls[&DETAIL], wide(&self.snapshot.detail).as_ptr());
        let label = match self.snapshot.calibration {
            1 => "记录：已开启",
            2 => "记录：已关闭",
            3 => "记录：已开启",
            _ => "校验注册表…",
        };
        SetWindowTextW(self.controls[&CALIBRATE], wide(label).as_ptr());
        self.tray(hwnd, false);
    }
    unsafe fn tray(&mut self, hwnd: HWND, add: bool) {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY;
        data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        let tip = wide(&format!(
            "Replay Guardian：{} · {}{}",
            self.snapshot.state.label(),
            self.snapshot.actual.label(),
            if self.paused { " · 已暂停" } else { "" }
        ));
        let n = tip.len().min(data.szTip.len() - 1);
        data.szTip[..n].copy_from_slice(&tip[..n]);
        if Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &data) != 0 {
            self.tray_added = true;
        }
    }
    unsafe fn balloon(&self, hwnd: HWND) {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
        let title = wide("Replay Guardian");
        data.szInfoTitle[..title.len()].copy_from_slice(&title);
        let message = wide("已确认即时回放恢复开启。");
        data.szInfo[..message.len()].copy_from_slice(&message);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
    unsafe fn menu(&mut self, hwnd: HWND) {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (id, label, flags) in [
            (
                0,
                format!("即时回放：{}", self.snapshot.state.label()),
                MF_GRAYED,
            ),
            (
                0,
                format!("检测方式：{}", self.snapshot.actual.label()),
                MF_GRAYED,
            ),
            (
                PAUSE,
                if self.paused {
                    "恢复监控"
                } else {
                    "暂停监控"
                }
                .into(),
                MF_STRING,
            ),
            (SETTINGS, "打开设置".into(), MF_STRING),
            (EXIT, "退出程序".into(), MF_STRING),
        ] {
            AppendMenuW(menu, flags, id as usize, wide(&label).as_ptr());
        }
        let mut point: POINT = std::mem::zeroed();
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        if command != 0 {
            PostMessageW(hwnd, WM_COMMAND, command as usize, 0);
        }
        PostMessageW(hwnd, WM_NULL, 0, 0);
    }
    unsafe fn change(&mut self, hwnd: HWND) {
        let mut c = self.config.clone();
        c.mode =
            [Mode::Auto, Mode::Mcp, Mode::Poll, Mode::Event][selected(self.controls[&MODE]).min(3)];
        c.interval = [5, 10, 30, 60][selected(self.controls[&INTERVAL]).min(3)];
        c.retry = [30, 60, 120, 300][selected(self.controls[&RETRY]).min(3)];
        c.restore = checked(self.controls[&RESTORE]);
        c.startup = checked(self.controls[&STARTUP]);
        c.notify = checked(self.controls[&NOTIFY]);
        c.alt = checked(self.controls[&ALT]);
        c.shift = checked(self.controls[&SHIFT]);
        c.ctrl = checked(self.controls[&CTRL]);
        let keys = config::key_options();
        c.key = keys[selected(self.controls[&KEY]).min(keys.len() - 1)].0;
        if self.safe {
            c.restore = false;
            c.startup = false;
        }
        if let Err(e) = c.validate() {
            SetWindowTextW(self.controls[&DETAIL], wide(&e).as_ptr());
            return;
        }
        if c.startup != self.config.startup
            && let Err(e) = config::set_startup(c.startup)
        {
            checkbox(self.controls[&STARTUP], self.config.startup);
            MessageBoxW(
                hwnd,
                wide(&e).as_ptr(),
                wide("Replay Guardian").as_ptr(),
                MB_ICONERROR,
            );
            return;
        }
        if !self.safe
            && let Err(e) = c.save()
        {
            MessageBoxW(
                hwnd,
                wide(&e).as_ptr(),
                wide("保存失败").as_ptr(),
                MB_ICONERROR,
            );
        }
        self.config = c;
        if let Some(w) = &self.worker {
            w.send(Command::Configure(self.config.clone(), self.paused));
        }
    }
}
unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(l as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return DefWindowProcW(hwnd, message, w, l);
    }
    if message == WM_APP + 3 {
        DestroyWindow(hwnd);
        return 0;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    let Ok(mut app) = (*ptr).try_borrow_mut() else {
        return DefWindowProcW(hwnd, message, w, l);
    };
    if message == app.taskbar && app.taskbar != 0 {
        app.tray(hwnd, true);
        return 0;
    }
    match message {
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_DESTROY => {
            let mut data: NOTIFYICONDATAW = std::mem::zeroed();
            data.cbSize = std::mem::size_of_val(&data) as u32;
            data.hWnd = hwnd;
            data.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &data);
            app.worker.take();
            PostQuitMessage(0);
            0
        }
        monitor::UPDATE => {
            app.update(hwnd);
            0
        }
        TRAY => {
            if l as u32 == WM_RBUTTONUP {
                app.menu(hwnd);
            } else if l as u32 == WM_LBUTTONDBLCLK {
                if app.controls.is_empty() {
                    app.build(hwnd);
                    app.update(hwnd);
                }
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
            0
        }
        WM_COMMAND => {
            let id = (w & 0xffff) as u16;
            let notification = (w >> 16) as u16;
            match id {
                EXIT => {
                    if let Some(worker) = &app.worker {
                        worker.send(Command::Exit);
                    }
                    PostMessageW(hwnd, WM_APP + 3, 0, 0);
                }
                SETTINGS => {
                    if app.controls.is_empty() {
                        app.build(hwnd);
                        app.update(hwnd);
                    }
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
                PAUSE => {
                    app.paused = !app.paused;
                    if let Some(worker) = &app.worker {
                        worker.send(Command::Configure(app.config.clone(), app.paused));
                    }
                    app.update(hwnd);
                }
                TEST => {
                    if let Some(worker) = &app.worker {
                        worker.send(Command::Test);
                    }
                    SetWindowTextW(
                        app.controls[&DETAIL],
                        wide("正在测试（仅检测，不主动切换回放）…").as_ptr(),
                    );
                }
                CALIBRATE => {
                    if app.snapshot.calibration == 0
                        && MessageBoxW(hwnd,wide("请在 NVIDIA 中手动开启即时回放，然后点击确定。接着按提示手动关闭、再开启并分别记录。校验期间暂停自动恢复。\n\n取消校验请点击窗口中的“取消校验”。").as_ptr(),wide("注册表本机校验").as_ptr(),MB_OKCANCEL|MB_ICONINFORMATION)!=IDOK{return 0;}
                    if let Some(worker) = &app.worker {
                        worker.send(Command::Calibrate);
                    }
                }
                CANCEL => {
                    if let Some(worker) = &app.worker {
                        worker.send(Command::CancelCalibration);
                    }
                }
                MODE | INTERVAL | RETRY | KEY if notification == CBN_SELCHANGE as u16 => {
                    app.change(hwnd)
                }
                RESTORE | STARTUP | NOTIFY | ALT | SHIFT | CTRL
                    if notification == BN_CLICKED as u16 =>
                {
                    app.change(hwnd)
                }
                _ => {}
            }
            0
        }
        _ => DefWindowProcW(hwnd, message, w, l),
    }
}
pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|a| a == "--diagnose") {
        let path = args.get(index + 1).ok_or("--diagnose 需要输出文件路径")?;
        let result = match crate::detector::mcp::Mcp::connect() {
            Ok(mut c) => format!(
                "MCP state: {:?}\nEnable tool available: {}\n",
                c.status(),
                c.can_enable
            ),
            Err(e) => format!("MCP error: {e}\n"),
        };
        let report = format!(
            "{result}Registry candidate: {:?}\nRegistry fingerprint: {:?}\n",
            crate::detector::registry::read(),
            crate::detector::registry::fingerprint()
        );
        std::fs::write(path, report).map_err(|e| e.to_string())?;
        return Ok(());
    }
    unsafe {
        let mutex = Handle(CreateMutexW(
            null(),
            0,
            wide("Local\\ReplayGuardian.Monitor.v1").as_ptr(),
        ));
        if mutex.0.is_null() {
            return Err(last_error("单实例检查"));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let hwnd = FindWindowW(wide("ReplayGuardianWindow").as_ptr(), null());
            if !hwnd.is_null() {
                PostMessageW(hwnd, WM_COMMAND, SETTINGS as usize, 0);
                SetForegroundWindow(hwnd);
            }
            return Ok(());
        }
        SetProcessDPIAware();
        let instance = GetModuleHandleW(null());
        let class = wide("ReplayGuardianWindow");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        wc.hbrBackground = (COLOR_BTNFACE + 1) as usize as _;
        if RegisterClassW(&wc) == 0 {
            return Err(last_error("注册窗口"));
        }
        let (mut config, warning) = Config::load();
        let safe = args.iter().any(|a| a == "--safe");
        if safe {
            config.restore = false;
            config.startup = false;
        }
        let dc = GetDC(null_mut());
        let dpi = if dc.is_null() {
            96
        } else {
            let d = GetDeviceCaps(dc, LOGPIXELSX as i32);
            ReleaseDC(null_mut(), dc);
            d
        };
        let scale = dpi as f64 / 96.0;
        let mut app = Box::new(RefCell::new(App {
            config: config.clone(),
            paused: false,
            snapshot: Snapshot::default(),
            worker: None,
            controls: BTreeMap::new(),
            taskbar: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            tray_added: false,
            scale,
            safe,
        }));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(if safe {
                "Replay Guardian（只读测试）"
            } else {
                "Replay Guardian"
            })
            .as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (530.0 * scale) as i32,
            (630.0 * scale) as i32,
            null_mut(),
            null_mut(),
            instance,
            (&mut *app as *mut RefCell<App>).cast(),
        );
        if hwnd.is_null() {
            return Err(last_error("创建设置窗口"));
        }
        {
            let mut state = app.borrow_mut();
            if !args.iter().any(|a| a == "--background") {
                state.build(hwnd);
            }
            state.worker = Some(Worker::start(hwnd as usize, config)?);
            state.tray(hwnd, true);
        }
        // Consume a launcher's hidden first-show instruction before later tray opens.
        ShowWindow(hwnd, SW_HIDE);
        if !args.iter().any(|a| a == "--background") || !app.borrow().tray_added {
            ShowWindow(hwnd, SW_SHOW);
        }
        if let Some(warning) = warning {
            MessageBoxW(
                hwnd,
                wide(&warning).as_ptr(),
                wide("配置提示").as_ptr(),
                MB_ICONWARNING,
            );
        }
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result == 0 {
                break;
            }
            if result < 0 {
                return Err(last_error("窗口消息读取"));
            }
            if IsDialogMessageW(hwnd, &msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        drop(app);
        drop(mutex);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_window_title_controls_close_and_exit() {
        unsafe {
            let instance = GetModuleHandleW(null());
            let class = wide("ReplayGuardianNativeTestWindow");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            assert_ne!(RegisterClassW(&wc), 0);
            let mut app = Box::new(RefCell::new(App {
                config: Config {
                    restore: false,
                    ..Config::default()
                },
                paused: false,
                snapshot: Snapshot::default(),
                worker: None,
                controls: BTreeMap::new(),
                taskbar: 0,
                tray_added: false,
                scale: 1.0,
                safe: true,
            }));
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Replay Guardian test").as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                530,
                630,
                null_mut(),
                null_mut(),
                instance,
                (&mut *app as *mut RefCell<App>).cast(),
            );
            assert!(!hwnd.is_null());
            app.borrow_mut().build(hwnd);
            assert!(GetWindowTextLengthW(hwnd) > 0);
            assert_eq!(
                SendMessageW(app.borrow().controls[&MODE], CB_GETCOUNT, 0, 0),
                4
            );
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            assert_ne!(IsWindow(hwnd), 0);
            assert_eq!(IsWindowVisible(hwnd), 0);
            PostMessageW(hwnd, WM_COMMAND, EXIT as usize, 0);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            assert_eq!(IsWindow(hwnd), 0);
            UnregisterClassW(class.as_ptr(), instance);
        }
    }
}
