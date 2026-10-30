use super::rate_limit::{self, Gate};
use crate::{
    detector::{State, parse_status},
    win::{last_error, wide},
};
use serde_json::{Value, json};
use std::os::windows::process::CommandExt;
use std::{
    ffi::c_void,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::Networking::WinHttp::*;
static CALL_GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
fn gate() -> std::sync::MutexGuard<'static, Gate> {
    CALL_GATE
        .get_or_init(|| Mutex::new(Gate::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
pub fn cooldown_remaining() -> Option<Duration> {
    gate().remaining(Instant::now())
}
pub fn tool_gap() -> Duration {
    gate().tool_gap()
}
pub fn is_rate_limited(error: &str) -> bool {
    error.starts_with(rate_limit::MESSAGE)
}
pub fn cooldown_message() -> String {
    rate_limit::message(cooldown_remaining().unwrap_or(Duration::from_secs(1)))
}
struct Internet(*mut c_void);
impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}
struct Http {
    session: Internet,
    connection: Internet,
    path: String,
}
struct Bridge {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Result<String, String>>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
enum Transport {
    Http(Http),
    Bridge(Bridge),
}
pub struct Mcp {
    transport: Transport,
    legacy: bool,
    session: Option<String>,
    id: u64,
    pub can_enable: bool,
    cancellation: Option<(Arc<crate::win::Handle>, Arc<AtomicU64>, u64)>,
}

fn endpoint() -> Result<(u16, String), String> {
    let file = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("NVIDIA Corporation/NvAppMcpServer/server.json");
    match std::fs::read(file) {
        Ok(data) => {
            let v: Value = serde_json::from_slice(&data).map_err(|_| "NVIDIA 端点文件损坏")?;
            let url = v["http"]
                .as_str()
                .ok_or("NVIDIA HTTP 不可用，可尝试官方桥接")?;
            parse_endpoint(url)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((13508, "/mcp".into())),
        Err(e) => Err(format!("无法读取 NVIDIA 端点文件：{e}")),
    }
}
fn parse_endpoint(url: &str) -> Result<(u16, String), String> {
    let tail = url
        .strip_prefix("http://127.0.0.1:")
        .ok_or("只允许 NVIDIA 本机 IPv4 回环端点")?;
    let (port, path) = tail.split_once('/').ok_or("端点路径缺失")?;
    if path != "mcp" && path != "mcp/" {
        return Err("NVIDIA 端点路径无效".into());
    }
    let port = port.parse::<u16>().map_err(|_| "端点端口无效")?;
    if port == 0 {
        return Err("端点端口无效".into());
    }
    Ok((port, format!("/{path}")))
}
fn header(request: *mut c_void, name: &str) -> Option<String> {
    unsafe {
        let mut buffer = [0u16; 1024];
        let mut len = std::mem::size_of_val(&buffer) as u32;
        if WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_CUSTOM,
            wide(name).as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
        ) == 0
        {
            return None;
        }
        Some(
            String::from_utf16_lossy(&buffer[..len as usize / 2])
                .trim_end_matches('\0')
                .to_string(),
        )
    }
}
impl Http {
    fn new() -> Result<Self, String> {
        let (port, path) = endpoint()?;
        Self::at(port, path)
    }
    fn at(port: u16, path: String) -> Result<Self, String> {
        unsafe {
            let session = Internet(WinHttpOpen(
                wide("ReplayGuardian/0.1").as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            ));
            if session.0.is_null() {
                return Err(last_error("WinHTTP"));
            }
            WinHttpSetTimeouts(session.0, 1500, 1500, 2500, 3000);
            let connection = Internet(WinHttpConnect(
                session.0,
                wide("127.0.0.1").as_ptr(),
                port,
                0,
            ));
            if connection.0.is_null() {
                return Err(last_error("连接 NVIDIA"));
            }
            Ok(Self {
                session,
                connection,
                path,
            })
        }
    }
    fn request(
        &self,
        body: &Value,
        legacy: bool,
        session: Option<&str>,
    ) -> Result<(Value, Option<String>), String> {
        unsafe {
            let _keep_alive = &self.session;
            let method = body["method"].as_str().unwrap_or("");
            let id = body["id"].as_u64();
            let request = Internet(WinHttpOpenRequest(
                self.connection.0,
                wide("POST").as_ptr(),
                wide(&self.path).as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            ));
            if request.0.is_null() {
                return Err(last_error("创建 MCP 请求"));
            }
            let flags = WINHTTP_DISABLE_REDIRECTS;
            if WinHttpSetOption(
                request.0,
                WINHTTP_OPTION_DISABLE_FEATURE,
                (&flags as *const u32).cast(),
                4,
            ) == 0
            {
                return Err(last_error("禁用重定向"));
            }
            let version = if legacy { "2025-11-25" } else { "2026-07-28" };
            let mut headers = format!(
                "Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {version}\r\nMcp-Method: {method}\r\n"
            );
            if let Some(name) = body["params"]["name"].as_str() {
                headers.push_str(&format!("Mcp-Name: {name}\r\n"));
            }
            if let Some(session) = session {
                if session.contains(['\r', '\n']) || session.len() > 512 {
                    return Err("会话标识无效".into());
                }
                headers.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
            }
            let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
            let h = wide(&headers);
            if WinHttpSendRequest(
                request.0,
                h.as_ptr(),
                (h.len() - 1) as u32,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                bytes.len() as u32,
                0,
            ) == 0
                || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0
            {
                return Err(last_error("MCP 请求失败"));
            }
            let mut status = 0u32;
            let mut length = 4;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                std::ptr::null(),
                (&mut status as *mut u32).cast(),
                &mut length,
                std::ptr::null_mut(),
            );
            if status == 401 || status == 403 {
                return Err(format!("MCP 权限受限 HTTP {status}，不会改用快捷键绕过"));
            }
            if status == 429 {
                let seconds = header(request.0, "Retry-After")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(30);
                return Ok((
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32029,"data":{"retryAfter":seconds},"message":"HTTP 429"}}),
                    None,
                ));
            }
            if status == 202 && id.is_none() {
                return Ok((Value::Null, header(request.0, "Mcp-Session-Id")));
            }
            let session = header(request.0, "Mcp-Session-Id");
            let mut response = Vec::new();
            let start = Instant::now();
            loop {
                let mut buffer = [0u8; 8192];
                let mut read = 0;
                if WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                ) == 0
                {
                    return Err(last_error("读取 MCP 响应"));
                }
                response.extend_from_slice(&buffer[..read as usize]);
                if response.len() > 1024 * 1024 {
                    return Err("MCP 响应超过 1 MB".into());
                }
                if let Some(value) = decode_response(&response, id)? {
                    return Ok((value, session));
                }
                if read == 0 {
                    return Err(format!("MCP HTTP {status}：响应不完整或请求 ID 不匹配"));
                }
                if start.elapsed() > Duration::from_secs(8) {
                    return Err("MCP 响应超时".into());
                }
            }
        }
    }
}
fn decode_response(bytes: &[u8], id: Option<u64>) -> Result<Option<Value>, String> {
    let matches = |v: &Value| {
        v.get("id").and_then(Value::as_u64) == id
            && (v.get("result").is_some() || v.get("error").is_some())
    };
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        if matches(&v) {
            return Ok(Some(v));
        }
        return Ok(None);
    }
    // Parse only complete SSE events; UTF-8 may be split across network reads.
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(None);
    };
    let normalized = text.replace("\r\n", "\n");
    for event in normalized.split_terminator("\n\n") {
        if !normalized.contains(&format!("{event}\n\n")) {
            continue;
        }
        let data = event
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|s| s.trim_start_matches(' '))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| "MCP SSE 数据不是有效 JSON")?;
        if matches(&value) {
            return Ok(Some(value));
        }
    }
    Ok(None)
}
impl Bridge {
    fn new() -> Result<Self, String> {
        let exe =
            std::path::PathBuf::from(std::env::var_os("ProgramFiles").ok_or("ProgramFiles 缺失")?)
                .join("NVIDIA Corporation/NVIDIA App/McpServer/NvAppMcpServer.exe");
        let mut child = Command::new(exe)
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("官方 MCP 桥接不可用：{e}"))?;
        let input = child.stdin.take().ok_or("桥接输入不可用")?;
        let stdout = child.stdout.take().ok_or("桥接输出不可用")?;
        let (tx, output) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                match (&mut r).take(1024 * 1024 + 1).read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(n) if n <= 1024 * 1024 => {
                        let message =
                            String::from_utf8(bytes).map_err(|_| "桥接输出不是 UTF-8".to_string());
                        if tx.send(message).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = tx.send(Err("桥接输出超过上限或读取失败".into()));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            output,
            reader: Some(reader),
        })
    }
    fn request(&mut self, body: &Value) -> Result<Value, String> {
        writeln!(self.input, "{body}")
            .and_then(|_| self.input.flush())
            .map_err(|e| format!("桥接写入失败：{e}"))?;
        if body.get("id").is_none() {
            return Ok(Value::Null);
        }
        let start = Instant::now();
        loop {
            let line = self
                .output
                .recv_timeout(Duration::from_secs(8).saturating_sub(start.elapsed()))
                .map_err(|_| "桥接响应超时或连接关闭")??;
            let value: Value = serde_json::from_str(&line).map_err(|_| "桥接 JSON 损坏")?;
            if value["id"] == body["id"] {
                return Ok(value);
            }
            if start.elapsed() > Duration::from_secs(8) {
                return Err("桥接响应超时".into());
            }
        }
    }
}
impl Mcp {
    pub fn connect() -> Result<Self, String> {
        if cooldown_remaining().is_some() {
            return Err(cooldown_message());
        }
        let http = Http::new().and_then(|http| Self::start(Transport::Http(http)));
        match http {
            Ok(c) => Ok(c),
            Err(e) if e.contains("权限") || is_rate_limited(&e) => Err(e),
            Err(http_error) => Self::start(Transport::Bridge(Bridge::new()?))
                .map_err(|e| format!("HTTP: {http_error}；桥接: {e}")),
        }
    }
    fn start(transport: Transport) -> Result<Self, String> {
        let mut c = Self {
            transport,
            legacy: false,
            session: None,
            id: 0,
            can_enable: false,
            cancellation: None,
        };
        let discovery = c.rpc("server/discover", json!({}));
        let modern = discovery.as_ref().is_ok_and(|v| {
            v["result"]["supportedVersions"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "2026-07-28"))
        });
        if !modern {
            if discovery
                .as_ref()
                .is_err_and(|e| e.contains("权限") || is_rate_limited(e))
            {
                return Err(discovery.err().unwrap());
            }
            c.legacy = true;
            let init=c.rpc("initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"ReplayGuardian","version":"0.1.0"}}))?;
            if init["result"]["protocolVersion"] != "2025-11-25" {
                return Err("MCP 不支持已实现的兼容协议".into());
            }
            c.notification("notifications/initialized")?;
        }
        let list = c.rpc("tools/list", json!({}))?;
        let tools = list["result"]["tools"]
            .as_array()
            .ok_or("MCP 工具列表缺失")?;
        if !tools
            .iter()
            .any(|t| t["name"] == "nvapp_overlay_get_status")
        {
            return Err("MCP 没有状态读取工具".into());
        }
        c.can_enable = tools.iter().any(|t| {
            t["name"] == "nvapp_overlay_capture"
                && t["inputSchema"]["properties"]["enable"]["type"] == "boolean"
                && t["inputSchema"]["properties"]["action"]["enum"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == "toggle_instant_replay"))
        });
        Ok(c)
    }
    fn rpc(&mut self, method: &str, mut params: Value) -> Result<Value, String> {
        self.wait_for_slot(method == "tools/call")?;
        self.id += 1;
        if !self.legacy {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"ReplayGuardian","version":"0.1.0"},"io.modelcontextprotocol/clientCapabilities":{}});
        }
        let body = json!({"jsonrpc":"2.0","id":self.id,"method":method,"params":params});
        let value = match &mut self.transport {
            Transport::Http(http) => {
                let (v, session) = http.request(&body, self.legacy, self.session.as_deref())?;
                if session.is_some() {
                    self.session = session;
                }
                v
            }
            Transport::Bridge(b) => b.request(&body)?,
        };
        if let Some(delay) = rate_limit::retry_after(&value) {
            gate().block(Instant::now(), delay);
            return Err(cooldown_message());
        }
        if value.get("error").is_some() {
            return Err(format!("MCP 协议拒绝：{}", value["error"]));
        }
        Ok(value)
    }
    pub fn set_cancellation(
        &mut self,
        event: Arc<crate::win::Handle>,
        epoch: Arc<AtomicU64>,
        generation: u64,
    ) {
        self.cancellation = Some((event, epoch, generation));
    }
    fn wait_for_slot(&self, tool: bool) -> Result<(), String> {
        loop {
            if self
                .cancellation
                .as_ref()
                .is_some_and(|(_, epoch, generation)| epoch.load(Ordering::SeqCst) != *generation)
            {
                return Err("MCP 请求已因暂停或设置更改取消".into());
            }
            let wait = gate().acquire(Instant::now(), tool);
            match wait {
                Ok(()) => return Ok(()),
                Err(delay) => {
                    if let Some((event, _, _)) = &self.cancellation {
                        let millis = (delay.as_millis() + 1).min(u32::MAX as u128 - 1) as u32;
                        let result = unsafe {
                            windows_sys::Win32::System::Threading::WaitForSingleObject(
                                event.0, millis,
                            )
                        };
                        if result == windows_sys::Win32::Foundation::WAIT_OBJECT_0
                            && self
                                .cancellation
                                .as_ref()
                                .is_some_and(|(_, epoch, generation)| {
                                    epoch.load(Ordering::SeqCst) == *generation
                                })
                        {
                            continue;
                        }
                        if result != windows_sys::Win32::Foundation::WAIT_TIMEOUT {
                            return Err("MCP 请求等待已取消".into());
                        }
                    } else {
                        std::thread::sleep(delay);
                    }
                }
            }
        }
    }
    fn notification(&mut self, method: &str) -> Result<(), String> {
        let body = json!({"jsonrpc":"2.0","method":method});
        match &mut self.transport {
            Transport::Http(h) => {
                h.request(&body, true, self.session.as_deref())?;
            }
            Transport::Bridge(b) => {
                b.request(&body)?;
            }
        }
        Ok(())
    }
    pub fn status(&mut self) -> Result<State, String> {
        parse_status(&self.rpc(
            "tools/call",
            json!({"name":"nvapp_overlay_get_status","arguments":{}}),
        )?)
    }
    pub fn enable(&mut self) -> Result<String, String> {
        if !self.can_enable {
            return Err("MCP 没有可用的明确开启工具".into());
        }
        let value=self.rpc("tools/call",json!({"name":"nvapp_overlay_capture","arguments":{"action":"toggle_instant_replay","enable":true}}))?;
        if value["result"]["isError"] == true {
            return Err(format!("恢复请求失败：{}", value["result"]["content"]));
        }
        Ok(value["result"]["content"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["text"].as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "开启请求已返回，结果仍需状态确认".into()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_request_does_not_take_a_network_slot() {
        let event = Arc::new(crate::win::Handle(unsafe {
            windows_sys::Win32::System::Threading::CreateEventW(
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
            )
        }));
        let epoch = Arc::new(AtomicU64::new(1));
        let mut client = Mcp {
            transport: Transport::Http(Http::at(9, "/mcp".into()).unwrap()),
            legacy: false,
            session: None,
            id: 0,
            can_enable: false,
            cancellation: None,
        };
        client.set_cancellation(event, epoch, 0);
        assert!(client.wait_for_slot(true).unwrap_err().contains("取消"));
    }
    #[test]
    fn native_http_reads_split_sse_and_sends_modern_metadata() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse::<usize>().unwrap();
                }
                headers.push_str(&line);
            }
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                headers
                    .to_lowercase()
                    .contains("mcp-protocol-version: 2026-07-28")
            );
            assert_eq!(
                request["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
                "2026-07-28"
            );
            let body=b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"structuredContent\":{\"instantReplayEnabled\":false}}}\r\n\r\n";
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            stream.write_all(&body[..17]).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            stream.write_all(&body[17..]).unwrap();
        });
        let mut client = Mcp {
            transport: Transport::Http(Http::at(port, "/mcp".into()).unwrap()),
            legacy: false,
            session: None,
            id: 0,
            can_enable: false,
            cancellation: None,
        };
        assert_eq!(client.status().unwrap(), State::Off);
        server.join().unwrap();
    }
    #[test]
    fn json_and_sse() {
        for text in [
            "{\"id\":1,\"result\":{}}",
            "data: {\"id\":1,\"result\":{}}\n\n",
            ":keepalive\r\n\r\ndata: {\"id\":1,\"result\":{}}\r\n\r\n",
        ] {
            assert!(decode_response(text.as_bytes(), Some(1)).unwrap().is_some());
        }
    }
    #[test]
    fn partial_event_and_wrong_id() {
        assert!(
            decode_response(b"data: {\"id\":1,\"result\":{}}\n", Some(1))
                .unwrap()
                .is_none()
        );
        assert!(
            decode_response(b"{\"id\":2,\"result\":{}}", Some(1))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn endpoint_rejects_remote_and_injection() {
        assert!(parse_endpoint("http://127.0.0.1:13508/mcp/").is_ok());
        for url in [
            "http://evil/mcp",
            "http://127.0.0.1:80/evil",
            "http://127.0.0.1:0/mcp",
            "http://127.0.0.1:80/mcp\r\n",
        ] {
            assert!(parse_endpoint(url).is_err());
        }
    }
}
