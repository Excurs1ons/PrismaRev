//! 最小 CDP（Chrome DevTools Protocol）客户端：给 MCP 提供「浏览器 MCP」式的
//! 页面操作能力。
//!
//! 只依赖 `std::net` + `serde_json`，所以手写了 WebSocket 客户端帧格式
//! （RFC 6455，客户端必须掩码、服务端不掩码，单帧文本消息足够）。
//!
//! 用法前提：编辑器（Tauri / WebView2）启动时带上
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};

/// 默认调试端口（WebView2 的 `--remote-debugging-port`）。
pub const DEFAULT_PORT: u16 = 9222;

pub struct CdpSession {
    stream: TcpStream,
    next_id: u64,
}

impl CdpSession {
    /// 连到 `http://127.0.0.1:<port>/json` 找到第一个 page target，再升级成
    /// WebSocket。
    pub fn attach(port: u16) -> Result<Self, String> {
        let target = find_page_target(port)?;
        let ws_url = target
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "target 没有 webSocketDebuggerUrl（WebView2 需要带 --remote-debugging-port 启动）"
                    .to_string()
            })?;
        let stream = connect_websocket(ws_url)?;
        let mut session = Self { stream, next_id: 1 };
        // 打开 Runtime 域，后续 evaluate 才有意义。
        session.call("Runtime.enable", json!({}))?;
        Ok(session)
    }

    /// 调一个 CDP 方法，等它自己的响应。
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "id": id, "method": method, "params": params });
        self.send_text(&message.to_string())?;
        loop {
            let text = self.read_text()?;
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(format!("CDP {method} 失败: {error}"));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            // 事件（没有 id）直接丢弃：工具都是同步一次性调用。
        }
    }

    /// 在页面里跑一段 JS，返回结果（按值返回，`awaitPromise` 打开）。
    pub fn evaluate(&mut self, expression: &str) -> Result<Value, String> {
        let result = self.call(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "returnByValue": true,
                "awaitPromise": true,
                "userGesture": true,
            }),
        )?;
        if let Some(exception) = result.get("exceptionDetails") {
            let text = exception
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(Value::as_str)
                .or_else(|| exception.get("text").and_then(Value::as_str))
                .unwrap_or("未知 JS 异常");
            return Err(format!("页面脚本异常: {text}"));
        }
        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let payload = text.as_bytes();
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x81); // FIN + text
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream
            .write_all(&frame)
            .map_err(|err| format!("WebSocket 写入失败: {err}"))
    }

    fn read_text(&mut self) -> Result<String, String> {
        loop {
            let mut header = [0u8; 2];
            self.read_exact(&mut header)?;
            let opcode = header[0] & 0x0F;
            let masked = header[1] & 0x80 != 0;
            let mut len = (header[1] & 0x7F) as usize;
            if len == 126 {
                let mut buf = [0u8; 2];
                self.read_exact(&mut buf)?;
                len = u16::from_be_bytes(buf) as usize;
            } else if len == 127 {
                let mut buf = [0u8; 8];
                self.read_exact(&mut buf)?;
                len = u64::from_be_bytes(buf) as usize;
            }
            let mask = if masked {
                let mut buf = [0u8; 4];
                self.read_exact(&mut buf)?;
                Some(buf)
            } else {
                None
            };
            let mut payload = vec![0u8; len];
            self.read_exact(&mut payload)?;
            if let Some(mask) = mask {
                for (i, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[i % 4];
                }
            }
            match opcode {
                0x1 => return String::from_utf8(payload).map_err(|err| err.to_string()),
                0x8 => return Err("WebSocket 被对端关闭".into()),
                0x9 => self.send_control(0xA, &payload)?, // ping → pong
                _ => continue,                            // binary / pong / continuation
            }
        }
    }

    fn send_control(&mut self, opcode: u8, payload: &[u8]) -> Result<(), String> {
        let mut frame = vec![0x80 | opcode, 0x80 | payload.len() as u8, 1, 2, 3, 4];
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ [1u8, 2, 3, 4][i % 4]),
        );
        self.stream
            .write_all(&frame)
            .map_err(|err| format!("WebSocket 控制帧失败: {err}"))
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), String> {
        self.stream
            .read_exact(buf)
            .map_err(|err| format!("WebSocket 读取失败: {err}"))
    }
}

/// `GET /json` → 第一个 `type == "page"` 的 target。
fn find_page_target(port: u16) -> Result<Value, String> {
    let body = http_get(port, "/json")?;
    let targets: Vec<Value> = serde_json::from_str(&body).map_err(|err| {
        format!("调试端口返回的不是 JSON（端口 {port} 上真的是 WebView2 吗？）: {err}")
    })?;
    targets
        .into_iter()
        .find(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .ok_or_else(|| {
            format!("调试端口 {port} 上没有 page target —— 编辑器启动时带上 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port={port}")
        })
}

fn http_get(port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|err| {
        format!(
            "连不上 127.0.0.1:{port}（{err}）—— 编辑器需要带 --remote-debugging-port={port} 启动"
        )
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|err| err.to_string())?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("HTTP 写入失败: {err}"))?;

    // 不能 read_to_string：调试端口默认 keep-alive，等 EOF 会一直挂到超时。
    // 先读到头部结束，再按 Content-Length 精确读 body。
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .map_err(|err| format!("HTTP 读取失败: {err}"))?;
        head.push(byte[0]);
        if head.len() > 16384 {
            return Err("HTTP 响应头过长".into());
        }
    }
    let head = String::from_utf8_lossy(&head).to_string();
    let mut content_length: Option<usize> = None;
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = Vec::new();
    match content_length {
        Some(len) => {
            body.resize(len, 0);
            stream
                .read_exact(&mut body)
                .map_err(|err| format!("HTTP body 读取失败: {err}"))?;
        }
        None => {
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
            body = rest;
        }
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

fn connect_websocket(url: &str) -> Result<TcpStream, String> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| format!("只支持 ws:// 调试地址，收到 '{url}'"))?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| format!("调试地址端口不合法: '{authority}'"))?,
        ),
        None => (authority, 80),
    };

    let mut stream =
        TcpStream::connect((host, port)).map_err(|err| format!("连不上 {host}:{port}: {err}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|err| err.to_string())?;

    // 固定 key 也能通过：服务端只回显 Sec-WebSocket-Accept = base64(sha1(key+GUID))，
    // 我们不需要校验它。
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("WebSocket 握手写入失败: {err}"))?;

    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream
            .read_exact(&mut byte)
            .map_err(|err| format!("WebSocket 握手读取失败: {err}"))?;
        head.push(byte[0]);
        if head.len() > 8192 {
            return Err("WebSocket 握手响应过长".into());
        }
    }
    let head = String::from_utf8_lossy(&head);
    if !head.starts_with("HTTP/1.1 101") {
        return Err(format!(
            "WebSocket 握手被拒: {}",
            head.lines().next().unwrap_or_default()
        ));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_port_reports_actionable_error() {
        // 端口 1 基本不可能有服务在听；错误信息必须告诉用户怎么启动。
        let err = CdpSession::attach(1).err().expect("端口 1 不该有服务");
        assert!(err.contains("remote-debugging-port"), "{err}");
    }
}
