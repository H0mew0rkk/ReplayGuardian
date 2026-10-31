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
const SHOW_SETTINGS: u32 = WM_APP + 4;
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
    theme: Option<crate::ui_style::Theme>,
    icon: crate::tray::Icon,
    #[cfg(test)]
    test_tray: Option<bool>,
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
        let scale = self.scale;
        self.theme = Some(crate::ui_style::Theme::new(scale));
        let theme = self.theme.as_ref().unwrap();
        let state = self.snapshot.state;
        let paused = self.paused;
        let mut controls = BTreeMap::new();
        let mut put = |id: u16,
                       class: &str,
                       text: &str,
                       x: i32,
                       y: i32,
                       w: i32,
                       h: i32,
                       style: u32,
                       font: HFONT| {
            let c = control(hwnd, class, text, id, x, y, w, h, style, scale);
            theme.assign_control(c, id, y, state, paused);
            SendMessageW(c, WM_SETFONT, font as usize, 1);
            if class == "BUTTON" {
                SetWindowTheme(c, wide("Explorer").as_ptr(), null());
            }
            if id != 0 {
                controls.insert(id, c);
            }
            c
        };
        put(
            0,
            "STATIC",
            "Replay Guardian",
            28,
            18,
            460,
            34,
            0,
            theme.title,
        );
        put(
            STATUS,
            "STATIC",
            "正在读取回放状态",
            72,
            96,
            365,
            29,
            0,
            theme.status,
        );
        put(
            118,
            "STATIC",
            "等待检测方式就绪",
            72,
            128,
            365,
            20,
            0,
            theme.small,
        );
        put(
            PAUSE,
            "BUTTON",
            "暂停监控",
            464,
            102,
            124,
            34,
            WS_TABSTOP,
            theme.body,
        );
        for (title, y) in [("检测设置", 184), ("自动恢复", 314), ("常规选项", 442)] {
            put(0, "STATIC", title, 44, y, 400, 24, 0, theme.section);
        }
        put(0, "STATIC", "检测方式", 44, 219, 100, 24, 0, theme.body);
        let c = put(
            MODE,
            "COMBOBOX",
            "",
            170,
            214,
            422,
            200,
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
            theme.body,
        );
        for label in [
            "自动选择（推荐）",
            "NVIDIA MCP",
            "注册表轮询",
            "注册表事件监听",
        ] {
            SendMessageW(c, CB_ADDSTRING, 0, wide(label).as_ptr() as isize);
        }
        SendMessageW(
            c,
            CB_SETCURSEL,
            match self.config.mode {
                Mode::Auto => 0,
                Mode::Mcp => 1,
                Mode::Poll => 2,
                Mode::Event => 3,
            },
            0,
        );

        for (id, x, values, current) in [
            (INTERVAL, 170, vec![5, 10, 30, 60], self.config.interval),
            (RETRY, 462, vec![30, 60, 120, 300], self.config.retry),
        ] {
            put(
                0,
                "STATIC",
                if id == INTERVAL {
                    "检测间隔"
                } else {
                    "重试间隔"
                },
                if id == INTERVAL { 44 } else { 348 },
                258,
                105,
                24,
                0,
                theme.body,
            );
            let c = put(
                id,
                "COMBOBOX",
                "",
                x,
                253,
                130,
                200,
                CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
                theme.body,
            );
            for value in &values {
                SendMessageW(
                    c,
                    CB_ADDSTRING,
                    0,
                    wide(&format!("{value} 秒")).as_ptr() as isize,
                );
            }
            SendMessageW(
                c,
                CB_SETCURSEL,
                values.iter().position(|v| *v == current).unwrap_or(0),
                0,
            );
        }
        let c = put(
            RESTORE,
            "BUTTON",
            "自动恢复即时回放",
            44,
            340,
            232,
            28,
            BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
            theme.body,
        );
        checkbox(c, self.config.restore);
        put(
            119,
            "STATIC",
            "需要关闭回放时，请先暂停监控",
            293,
            346,
            300,
            20,
            0,
            theme.small,
        );
        put(0, "STATIC", "备用快捷键", 44, 387, 115, 24, 0, theme.body);
        for (id, label, x, value) in [
            (ALT, "Alt", 170, self.config.alt),
            (SHIFT, "Shift", 246, self.config.shift),
            (CTRL, "Ctrl", 338, self.config.ctrl),
        ] {
            let c = put(
                id,
                "BUTTON",
                label,
                x,
                380,
                80,
                30,
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                theme.body,
            );
            checkbox(c, value);
        }
        let c = put(
            KEY,
            "COMBOBOX",
            "",
            480,
            380,
            112,
            200,
            CBS_DROPDOWNLIST as u32 | WS_TABSTOP | WS_VSCROLL,
            theme.body,
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
        for (id, label, x, w, value) in [
            (STARTUP, "登录 Windows 时启动", 44, 250, self.config.startup),
            (NOTIFY, "恢复成功时通知", 320, 272, self.config.notify),
        ] {
            let c = put(
                id,
                "BUTTON",
                label,
                x,
                470,
                w,
                28,
                BS_AUTOCHECKBOX as u32 | WS_TABSTOP,
                theme.body,
            );
            checkbox(c, value);
        }
        for (id, label, x, w) in [
            (TEST, "测试检测方式", 28, 150),
            (CALIBRATE, "校验注册表…", 190, 160),
            (CANCEL, "取消校验", 362, 116),
            (EXIT, "退出程序", 490, 122),
        ] {
            put(id, "BUTTON", label, x, 548, w, 32, WS_TABSTOP, theme.body);
        }
        put(
            DETAIL,
            "EDIT",
            "监控诊断信息将在这里显示",
            28,
            592,
            584,
            64,
            ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL,
            theme.small,
        );
        self.controls = controls;
        SendMessageW(
            self.controls[&DETAIL],
            EM_SETMARGINS,
            (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
            ((8.0 * scale) as usize | (((8.0 * scale) as usize) << 16)) as isize,
        );
        EnableWindow(self.controls[&CANCEL], 0);
        if self.safe {
            for id in [RESTORE, STARTUP, CALIBRATE, CANCEL] {
                EnableWindow(self.controls[&id], 0);
            }
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
        let text = if self.paused {
            "监控已暂停".into()
        } else if self.snapshot.state == crate::detector::State::Unknown {
            "回放状态未知".into()
        } else {
            format!(
                "即时回放{}",
                self.snapshot.state.label().trim_start_matches("状态")
            )
        };
        self.tray(hwnd, false);
        if self.controls.is_empty() {
            if self.tray_added {
                return;
            }
            self.build(hwnd);
        }
        if let Some(theme) = &self.theme {
            for id in [STATUS, 118] {
                theme.assign_control(
                    self.controls[&id],
                    id,
                    100,
                    self.snapshot.state,
                    self.paused,
                );
            }
        }
        SetWindowTextW(self.controls[&STATUS], wide(&text).as_ptr());
        SetWindowTextW(
            self.controls[&118],
            wide(&format!(
                "当前检测：{}{}",
                self.snapshot.actual.label(),
                if self.safe { " · 只读测试" } else { "" }
            ))
            .as_ptr(),
        );
        SetWindowTextW(
            self.controls[&PAUSE],
            wide(if self.paused {
                "恢复监控"
            } else {
                "暂停监控"
            })
            .as_ptr(),
        );
        SetWindowTextW(self.controls[&DETAIL], wide(&self.snapshot.detail).as_ptr());
        let label = match self.snapshot.calibration {
            1 => "记录：已开启",
            2 => "记录：已关闭",
            3 => "记录：已开启",
            _ => "校验注册表…",
        };
        SetWindowTextW(self.controls[&CALIBRATE], wide(label).as_ptr());
        EnableWindow(
            self.controls[&CANCEL],
            i32::from(self.snapshot.calibration > 0 && !self.safe),
        );
        InvalidateRect(hwnd, null(), 0);
        if !self.tray_added {
            SetWindowTextW(
                self.controls[&DETAIL],
                wide("托盘图标暂不可用，设置窗口将保持可访问。可以直接点击“退出程序”。").as_ptr(),
            );
            if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }
    unsafe fn show_settings(&mut self, hwnd: HWND) {
        if self.controls.is_empty() {
            self.build(hwnd);
        }
        self.update(hwnd);
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
    unsafe fn tray(&mut self, hwnd: HWND, add: bool) -> bool {
        #[cfg(test)]
        if let Some(available) = self.test_tray {
            self.tray_added = available;
            return available;
        }
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of_val(&data) as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = TRAY;
        data.hIcon = self.icon.0;
        let tip = wide(&format!(
            "Replay Guardian：{} · {}{}",
            self.snapshot.state.label(),
            self.snapshot.actual.label(),
            if self.paused { " · 已暂停" } else { "" }
        ));
        let n = tip.len().min(data.szTip.len() - 1);
        data.szTip[..n].copy_from_slice(&tip[..n]);
        let previous = self.tray_added;
        self.tray_added = crate::tray::ensure_icon(previous, add, |operation| {
            Shell_NotifyIconW(operation, &data) != 0
        });
        if !self.tray_added && (add || previous) {
            config::log_error("系统托盘图标注册失败，保留设置窗口作为入口");
        }
        self.tray_added
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
    // Cross-instance activation can arrive while native code re-enters a borrowed
    // window callback. Queue it first instead of silently dropping the request.
    if message == WM_COMMAND && (w & 0xffff) as u16 == SETTINGS {
        PostMessageW(hwnd, SHOW_SETTINGS, 0, 0);
        return 1;
    }
    if matches!(
        message,
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN | WM_CTLCOLOREDIT
    ) && let Some(color) = crate::ui_style::control_color(l as HWND, w as HDC)
    {
        return color;
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RefCell<App>;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, message, w, l);
    }
    // DefWindowProc handles SC_CLOSE by synchronously sending WM_CLOSE. Holding
    // an App borrow across it made that nested callback fall through to default
    // destruction, leaving a windowless process holding the single-instance lock.
    if !matches!(
        message,
        SHOW_SETTINGS
            | WM_PAINT
            | WM_ERASEBKGND
            | WM_CLOSE
            | WM_DESTROY
            | monitor::UPDATE
            | TRAY
            | WM_COMMAND
    ) {
        let taskbar = (*ptr)
            .try_borrow()
            .is_ok_and(|app| app.taskbar != 0 && message == app.taskbar);
        if !taskbar {
            return DefWindowProcW(hwnd, message, w, l);
        }
    }
    let Ok(mut app) = (*ptr).try_borrow_mut() else {
        if message == WM_CLOSE {
            return 0;
        }
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }
        return DefWindowProcW(hwnd, message, w, l);
    };
    if message == app.taskbar && app.taskbar != 0 {
        app.tray_added = false;
        app.tray(hwnd, true);
        if !app.tray_added {
            app.show_settings(hwnd);
        }
        return 0;
    }
    match message {
        SHOW_SETTINGS => {
            app.show_settings(hwnd);
            1
        }
        WM_PAINT if app.theme.is_some() => {
            app.theme
                .as_ref()
                .unwrap()
                .paint(hwnd, app.scale, app.snapshot.state, app.paused);
            0
        }
        WM_ERASEBKGND if app.theme.is_some() => 1,
        WM_CLOSE => {
            if app.tray(hwnd, false) {
                ShowWindow(hwnd, SW_HIDE);
            } else {
                app.show_settings(hwnd);
            }
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
            } else if matches!(l as u32, WM_LBUTTONUP | WM_LBUTTONDBLCLK | NIN_SELECT) {
                app.show_settings(hwnd);
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
        _ => {
            drop(app);
            DefWindowProcW(hwnd, message, w, l)
        }
    }
}
unsafe fn activate_existing(class: &str) -> Result<(), String> {
    // Bounded startup-only wait: the mutex is created before its window exists.
    let mut hwnd = null_mut();
    for _ in 0..20 {
        hwnd = FindWindowW(wide(class).as_ptr(), null());
        if !hwnd.is_null() {
            break;
        }
        Sleep(100);
    }
    if hwnd.is_null() {
        return Err("已有 Replay Guardian 后台实例，但未找到设置窗口。可能是旧版残留进程；请在任务管理器结束 ReplayGuardian 后再打开新版。".into());
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    AllowSetForegroundWindow(pid);
    let mut result = 0;
    if SendMessageTimeoutW(
        hwnd,
        WM_COMMAND,
        SETTINGS as usize,
        0,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        2000,
        &mut result,
    ) == 0
    {
        return Err(
            "正在运行的 Replay Guardian 没有响应窗口打开请求。请结束旧实例后再打开新版。".into(),
        );
    }
    ShowWindowAsync(hwnd, SW_RESTORE);
    SetForegroundWindow(hwnd);
    for _ in 0..20 {
        if IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 {
            return Ok(());
        }
        Sleep(100);
    }
    Err("已向后台实例发送打开请求，但窗口仍未显示。请结束旧实例后重试。".into())
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
    let preview = args.iter().any(|arg| arg == "--preview");
    unsafe {
        let window_class = if preview {
            "ReplayGuardianPreviewWindow"
        } else {
            "ReplayGuardianWindow"
        };
        let mutex = Handle(CreateMutexW(
            null(),
            0,
            wide(if preview {
                "Local\\ReplayGuardian.Preview.v1"
            } else {
                "Local\\ReplayGuardian.Monitor.v1"
            })
            .as_ptr(),
        ));
        if mutex.0.is_null() {
            return Err(last_error("单实例检查"));
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return activate_existing(window_class);
        }
        SetProcessDPIAware();
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&controls);
        let instance = GetModuleHandleW(null());
        let icon = crate::tray::Icon::new()?;
        let class = wide(window_class);
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hIcon = icon.0;
        wc.hbrBackground = (COLOR_BTNFACE + 1) as usize as _;
        if RegisterClassW(&wc) == 0 {
            return Err(last_error("注册窗口"));
        }
        let (mut config, warning) = Config::load();
        let safe = preview || args.iter().any(|a| a == "--safe");
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
            theme: None,
            icon,
            #[cfg(test)]
            test_tray: None,
        }));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide(if safe {
                "Replay Guardian 0.1.3（只读测试）"
            } else {
                "Replay Guardian 0.1.3"
            })
            .as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ((crate::ui_style::CLIENT_WIDTH + 16) as f64 * scale) as i32,
            ((crate::ui_style::CLIENT_HEIGHT + 39) as f64 * scale) as i32,
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
            app.borrow_mut().show_settings(hwnd);
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
            if !(WM_KEYFIRST..=WM_KEYLAST).contains(&msg.message)
                || IsDialogMessageW(hwnd, &msg) == 0
            {
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
                theme: None,
                icon: crate::tray::Icon::new().unwrap(),
                test_tray: Some(false),
            }));
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Replay Guardian test").as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
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
            // Activation must survive re-entry while the app is borrowed and
            // create controls lazily for an initially background-only window.
            {
                let _borrow = app.borrow_mut();
                SendMessageW(hwnd, WM_COMMAND, SETTINGS as usize, 0);
            }
            let mut pending: MSG = std::mem::zeroed();
            while PeekMessageW(&mut pending, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&pending);
                DispatchMessageW(&pending);
            }
            assert_ne!(IsWindowVisible(hwnd), 0);
            assert!(GetWindowTextLengthW(hwnd) > 0);
            assert_eq!(
                SendMessageW(app.borrow().controls[&MODE], CB_GETCOUNT, 0, 0),
                4
            );
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE as usize, 0);
            assert_ne!(IsWindow(hwnd), 0);
            assert_ne!(
                IsWindowVisible(hwnd),
                0,
                "Tray failure must leave a usable window"
            );
            assert!(app.borrow().controls.contains_key(&EXIT));
            app.borrow_mut().test_tray = Some(true);
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_CLOSE as usize, 0);
            assert_ne!(
                IsWindow(hwnd),
                0,
                "Title-bar close must not destroy the hidden settings window"
            );
            assert_eq!(IsWindowVisible(hwnd), 0);
            ShowWindow(hwnd, SW_MINIMIZE);
            PostMessageW(hwnd, WM_COMMAND, SETTINGS as usize, 0);
            while PeekMessageW(&mut pending, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&pending);
                DispatchMessageW(&pending);
            }
            assert_eq!(
                IsIconic(hwnd),
                0,
                "Second launch must restore a minimized window"
            );
            assert_ne!(IsWindowVisible(hwnd), 0);
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
