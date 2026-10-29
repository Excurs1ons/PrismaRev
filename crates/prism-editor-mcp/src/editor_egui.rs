//! `editor_*` 工具：驱动**正在运行的** egui 编辑器（`editor-egui/`）。
//!
//! 编辑器在 `127.0.0.1:<port>` 上开一个本地控制通道，端口写在
//! `<项目根>/target/editor-egui/control.port`。这个模块只做 TCP + JSON，
//! 不链接 engine / render，也不开 Vulkan。
//!
//! | 工具 | 作用 |
//! |---|---|
//! | `editor_launch` | 起 `prism-editor-egui`（debug 构建），等控制通道就绪 |
//! | `editor_status` | 帧数 / 当前场景 / 选中 / Play 状态 |
//! | `editor_capture` | 抓整窗或某个视口的 PNG（`target` = `window` / `scene` / `game`） |
//! | `editor_quit` | 让编辑器退出 |
//! | `editor_ui` | UI 节点树快照 / 按节点点击 / 按节点设值（类似浏览器 MCP 的 DOM 操作） |

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::protocol::ToolDef;

/// 由 `editor_launch` 启动的进程。
#[derive(Default)]
pub struct EditorHost {
    child: Mutex<Option<Child>>,
}

impl EditorHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn call(&self, root: &Path, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "editor_launch" => self.launch(root, args),
            "editor_status" => status(root),
            "editor_capture" => capture(root, args),
            "editor_quit" => quit(self),
            "editor_ui" => ui(root, args),
            other => Err(format!("未知的编辑器工具 '{other}'")),
        }
    }

    fn launch(&self, root: &Path, args: &Value) -> Result<String, String> {
        let mut guard = self
            .child
            .lock()
            .map_err(|_| "editor child lock poisoned")?;
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(None) => return Err("编辑器已经在运行（先 editor_quit）".into()),
                _ => *guard = None,
            }
        }
        // 端口文件是「这一进程」的凭证：先删掉，等到它重新出现才算起好。
        let port_file = port_file(root);
        let _ = std::fs::remove_file(&port_file);

        let command = args
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| default_command(root));
        let mut parts = command.split_whitespace();
        let program = parts.next().ok_or("command 为空")?;
        let mut child = Command::new(program)
            .args(parts)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("启动 '{program}' 失败: {err}"))?;

        let wait = args
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(30_000);
        let deadline = Instant::now() + Duration::from_millis(wait);
        while Instant::now() < deadline {
            if port_file.exists() && control_port(root).is_ok() {
                // 端口一出现就返回会让第一条 `editor_ui` 拿到空 dock（首帧还没画面板），
                // 所以等到真正画出几帧再交还控制权。
                let ready = request(root, json!({ "cmd": "status" }))
                    .ok()
                    .and_then(|status| status.get("frames").and_then(Value::as_u64))
                    .map(|frames| frames >= 3)
                    .unwrap_or(false);
                if ready {
                    let pid = child.id();
                    *guard = Some(child);
                    return Ok(json!({
                        "ok": true,
                        "pid": pid,
                        "control": port_file.display().to_string(),
                        "command": command,
                    })
                    .to_string());
                }
            }
            if let Ok(Some(code)) = child.try_wait() {
                return Err(format!("编辑器退出（code {code}），控制通道没起来"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("等待控制通道超时（{wait} ms）"))
    }
}

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "editor_launch",
            description: "启动 egui 编辑器（默认 target/debug/prism-editor-egui），等本地控制通道就绪后返回 pid。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "覆盖启动命令（相对项目根）" },
                    "wait_ms": { "type": "integer", "default": 30000 }
                }
            }),
        },
        ToolDef {
            name: "editor_status",
            description: "正在运行的 egui 编辑器状态：帧数、当前场景、选中实体、Play / 暂停、是否有未保存改动。",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "editor_capture",
            description: "让正在运行的编辑器截图：target=window 抓整窗（UI + 视口），target=scene|game 抓该视口的引擎离屏图。返回 PNG 绝对路径。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["window", "scene", "game"],
                        "description": "抓什么：整窗 / Scene view / Game view，默认 window"
                    },
                    "path": { "type": "string", "description": "输出 PNG（相对项目根或绝对路径）；不给则写到 target/editor-egui/captures/" },
                    "size": { "type": "string", "description": "如 1920x1080（最近邻缩放在编辑器里做）" }
                }
            }),
        },
        ToolDef {
            name: "editor_ui",
            description: "编辑器 UI 节点树：action=snapshot 返回整棵界面树（面板 / 文档 / 层级实体 / 资产 / 检查器字段 / 命令表）；action=click 按节点 id 点击（command:Save / panel:hierarchy / doc:0 / entity:3 / asset:assets/x.prefab.json）；action=set 给检查器字段赋值（field:<entity>:<component>:<path>）。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["snapshot", "click", "set"], "default": "snapshot" },
                    "node": { "type": "string", "description": "click / set 时的节点 id（见 snapshot 返回）" },
                    "value": { "description": "set 时的值（JSON：数字 / 字符串 / bool / 数组）" }
                }
            }),
        },
        ToolDef {
            name: "editor_quit",
            description: "让编辑器走正常退出路径（会落盘 state.json）。",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
    ]
}

fn ui(root: &Path, args: &Value) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("snapshot");
    let (cmd, mut body) = match action {
        "snapshot" => ("ui.snapshot", json!({})),
        "click" => {
            let node = args
                .get("node")
                .and_then(Value::as_str)
                .ok_or("click 需要 node")?;
            ("ui.click", json!({ "node": node }))
        }
        "set" => {
            let node = args
                .get("node")
                .and_then(Value::as_str)
                .ok_or("set 需要 node")?;
            (
                "ui.set",
                json!({ "node": node, "value": args.get("value").cloned().unwrap_or(Value::Null) }),
            )
        }
        other => return Err(format!("未知 action '{other}'（snapshot / click / set）")),
    };
    body["cmd"] = json!(cmd);
    let response = request(root, body)?;
    if response.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(response.to_string());
    }
    Ok(response.to_string())
}

fn status(root: &Path) -> Result<String, String> {
    let response = request(root, json!({ "cmd": "status" }))?;
    Ok(response.to_string())
}

fn capture(root: &Path, args: &Value) -> Result<String, String> {
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or("window");
    if !matches!(target, "window" | "scene" | "game") {
        return Err(format!("未知 target '{target}'（window / scene / game）"));
    }
    let path = match args.get("path").and_then(Value::as_str) {
        Some(value) => {
            let value = PathBuf::from(value);
            if value.is_absolute() {
                value
            } else {
                root.join(value)
            }
        }
        None => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis())
                .unwrap_or_default();
            root.join("target/editor-egui/captures")
                .join(format!("{target}-{stamp}.png"))
        }
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("建目录失败: {err}"))?;
    }
    let mut request_body =
        json!({ "cmd": "capture", "target": target, "path": path.to_string_lossy() });
    if let Some(size) = args.get("size").and_then(Value::as_str) {
        request_body["size"] = json!(size);
    }
    let response = request(root, request_body)?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response.to_string());
    }
    Ok(response.to_string())
}

fn quit(host: &EditorHost) -> Result<String, String> {
    // 控件通道按 `--project` 找不到项目根时也能用：直接用已知端口文件。
    let root = crate::resolve_root().map_err(|err| err.to_string())?;
    let response = request(&root, json!({ "cmd": "quit" }))?;
    let mut guard = host
        .child
        .lock()
        .map_err(|_| "editor child lock poisoned")?;
    if let Some(mut child) = guard.take() {
        let _ = child.wait();
    }
    Ok(response.to_string())
}

/// 编辑器把端口写到这个文件（`PRISMREV_EDITOR_CONTROL` 可覆盖成端口号）。
fn port_file(root: &Path) -> PathBuf {
    match std::env::var_os("PRISMREV_EDITOR_CONTROL") {
        Some(value) => PathBuf::from(value),
        None => root.join("target/editor-egui/control.port"),
    }
}

fn control_port(root: &Path) -> Result<u16, String> {
    if let Ok(value) = std::env::var("PRISMREV_EDITOR_CONTROL") {
        if let Ok(port) = value.trim().parse::<u16>() {
            return Ok(port);
        }
    }
    let file = port_file(root);
    let text = std::fs::read_to_string(&file).map_err(|err| {
        format!(
            "读不到控制端口 {}: {err}（编辑器没开，或先 editor_launch）",
            file.display()
        )
    })?;
    text.trim()
        .parse::<u16>()
        .map_err(|err| format!("控制端口文件内容无效: {err}"))
}

fn request(root: &Path, body: Value) -> Result<Value, String> {
    let port = control_port(root)?;
    let stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|err| format!("连不上编辑器控制通道 127.0.0.1:{port}: {err}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut writer = stream
        .try_clone()
        .map_err(|err| format!("复制 socket 失败: {err}"))?;
    writeln!(writer, "{body}").map_err(|err| format!("写控制通道失败: {err}"))?;
    writer.flush().ok();
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|err| format!("读控制通道失败: {err}"))?;
    serde_json::from_str(line.trim()).map_err(|err| format!("控制通道回执不是 JSON: {err}"))
}

fn default_command(root: &Path) -> String {
    let debug = root.join("editor-egui/target/debug/prism-editor-egui");
    let release = root.join("editor-egui/target/release/prism-editor-egui");
    for candidate in [release, debug] {
        let with_exe = if cfg!(windows) {
            candidate.with_extension("exe")
        } else {
            candidate.clone()
        };
        if with_exe.exists() {
            return with_exe.display().to_string();
        }
    }
    // 没构建过就退回 cargo（会编译，慢但可用）。
    "cargo run -p prism-editor-egui --manifest-path editor-egui/Cargo.toml".to_string()
}

/// 端口文件存在但编辑器已经死了 → 清掉陈旧文件（`editor_launch` 前用）。
#[allow(dead_code)]
pub fn stale_port_file(root: &Path) -> bool {
    let file = port_file(root);
    if let Ok(text) = std::fs::read_to_string(&file) {
        if let Ok(port) = text.trim().parse::<u16>() {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return false;
            }
        }
    }
    let _ = std::fs::remove_file(&file);
    true
}

#[allow(dead_code)]
fn free_port_probe() -> Option<u16> {
    TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| addr.port())
}
