//! 编辑器 UI 工具：像浏览器 MCP 那样把页面里的按钮 / 文本 / 输入框列出来，
//! 并支持点击、改值、高亮。
//!
//! 实现方式是 CDP：`prism-editor-mcp` 连到 Tauri（WebView2）的调试端口，
//! 在页面里执行一段 DOM 脚本。所以**不需要改编辑器代码**，也不用重新构建前端。
//!
//! 前提：编辑器启动时带
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`。

use serde_json::{json, Value};

use crate::cdp::{CdpSession, DEFAULT_PORT};
use crate::protocol::ToolDef;

pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "ui_snapshot",
            description: "列出编辑器页面上的可交互元素与文本（按钮 / 输入框 / 下拉 / 文本），带稳定 id 与屏幕坐标。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "只保留标签或文本包含该子串的节点" },
                    "max_nodes": { "type": "integer", "description": "上限，默认 400" },
                    "port": { "type": "integer", "description": "调试端口，默认 9222" }
                }
            }),
        },
        ToolDef {
            name: "ui_click",
            description: "点击一个元素（先 scrollIntoView，再派发完整指针/鼠标事件序列）。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "ui_snapshot 返回的 id" },
                    "port": { "type": "integer" }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "ui_fill",
            description: "给输入框 / 文本域赋值，并派发 input + change（Vue 的 v-model 能收到）。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "value": { "description": "字符串；复选框用 true/false" },
                    "port": { "type": "integer" }
                },
                "required": ["id", "value"]
            }),
        },
        ToolDef {
            name: "ui_highlight",
            description: "在元素上画一个高亮框（默认 1500ms），用于把注意力指给用户。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "ms": { "type": "integer", "description": "持续时间，默认 1500" },
                    "port": { "type": "integer" }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "ui_eval",
            description: "在页面里执行一段 JS 并返回值（逃生口：快照覆盖不到的情况用它）。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "表达式或 IIFE" },
                    "port": { "type": "integer" }
                },
                "required": ["script"]
            }),
        },
        ToolDef {
            name: "ui_screenshot",
            description: "截取编辑器窗口（CDP Page.captureScreenshot），写 PNG。",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "port": { "type": "integer" }
                },
                "required": ["path"]
            }),
        },
    ]
}

pub fn call(name: &str, args: &Value) -> Result<String, String> {
    let port = args
        .get("port")
        .and_then(Value::as_u64)
        .map(|port| port as u16)
        .unwrap_or(DEFAULT_PORT);
    match name {
        "ui_snapshot" => {
            let max_nodes = args
                .get("max_nodes")
                .and_then(Value::as_u64)
                .unwrap_or(400)
                .min(5000);
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let script = snapshot_script(max_nodes, query);
            let mut session = CdpSession::attach(port)?;
            let value = session.evaluate(&script)?;
            serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
        }
        "ui_click" => {
            let id = str_arg(args, "id")?;
            let mut session = CdpSession::attach(port)?;
            let value = session.evaluate(&click_script(&id))?;
            serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
        }
        "ui_fill" => {
            let id = str_arg(args, "id")?;
            let value = args
                .get("value")
                .ok_or_else(|| "缺少 'value'".to_string())?
                .clone();
            let mut session = CdpSession::attach(port)?;
            let result = session.evaluate(&fill_script(&id, &value))?;
            serde_json::to_string_pretty(&result).map_err(|err| err.to_string())
        }
        "ui_highlight" => {
            let id = str_arg(args, "id")?;
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(1500);
            let mut session = CdpSession::attach(port)?;
            let result = session.evaluate(&highlight_script(&id, ms))?;
            serde_json::to_string_pretty(&result).map_err(|err| err.to_string())
        }
        "ui_eval" => {
            let script = str_arg(args, "script")?;
            let mut session = CdpSession::attach(port)?;
            let value = session.evaluate(&script)?;
            serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
        }
        "ui_screenshot" => {
            let path = str_arg(args, "path")?;
            let mut session = CdpSession::attach(port)?;
            let result = session.call("Page.captureScreenshot", json!({ "format": "png" }))?;
            let data = result
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| "captureScreenshot 没有返回 data".to_string())?;
            let bytes = base64_decode(data)?;
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::write(&path, &bytes).map_err(|err| format!("写 {path} 失败: {err}"))?;
            Ok(serde_json::to_string_pretty(&json!({
                "path": path,
                "bytes": bytes.len(),
            }))
            .unwrap_or_default())
        }
        other => Err(format!("未知工具 '{other}'")),
    }
}

fn str_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("缺少字符串参数 '{key}'"))
}

/// 页面里共用的取元素辅助函数（按 `data-prism-ui` 找）。
fn lookup(id: &str) -> String {
    format!(
        "(() => {{ const el = document.querySelector('[data-prism-ui=\"{}\"]'); \
         if (!el) throw new Error('找不到元素 {}，先跑一次 ui_snapshot'); return el; }})()",
        escape(id),
        escape(id)
    )
}

fn escape(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
}

fn snapshot_script(max_nodes: u64, query: &str) -> String {
    format!(
        r#"(() => {{
  const MAX = {max_nodes};
  const QUERY = "{query}".toLowerCase();
  let seq = window.__prismUiSeq || 0;
  const idOf = (el) => {{
    let id = el.getAttribute('data-prism-ui');
    if (!id) {{ seq += 1; id = 'u' + seq; el.setAttribute('data-prism-ui', id); }}
    return id;
  }};
  const visible = (el) => {{
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  }};
  const roleOf = (el) => {{
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'button') return 'button';
    if (tag === 'a') return 'link';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'input') {{
      const t = (el.type || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio') return t;
      if (t === 'range') return 'slider';
      if (t === 'color') return 'color';
      if (t === 'file') return 'file';
      return 'textbox';
    }}
    return 'text';
  }};
  const labelOf = (el) => {{
    const raw = el.getAttribute('aria-label') || el.getAttribute('title') ||
      el.getAttribute('placeholder') || el.innerText || el.value || '';
    return String(raw).replace(/\s+/g, ' ').trim().slice(0, 140);
  }};
  const interactive = (el) => {{
    const tag = el.tagName.toLowerCase();
    if (['button', 'a', 'input', 'select', 'textarea'].includes(tag)) return true;
    if (el.hasAttribute('role') || el.hasAttribute('data-prism-ui')) return true;
    if (el.hasAttribute('tabindex') && el.getAttribute('tabindex') !== '-1') return true;
    return false;
  }};
  const out = [];
  const walk = (el) => {{
    if (out.length >= MAX) return;
    if (!visible(el)) return;
    const role = roleOf(el);
    const label = labelOf(el);
    const isInteractive = interactive(el);
    // 文本节点：只收「自己带文字、且子元素没文字」的叶子，避免整棵树重复。
    const leafText = !isInteractive && label && !el.querySelector('button, input, select, textarea, a, [role]');
    if (isInteractive || (leafText && el.children.length === 0)) {{
      const hit = !QUERY || label.toLowerCase().includes(QUERY) || role.includes(QUERY);
      if (hit) {{
        const r = el.getBoundingClientRect();
        const node = {{
          id: idOf(el),
          role,
          tag: el.tagName.toLowerCase(),
          label,
          bounds: {{ x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) }},
        }};
        if ('value' in el && el.value !== undefined && el.value !== null) node.value = String(el.value).slice(0, 200);
        if (el.type === 'checkbox' || el.type === 'radio') node.checked = !!el.checked;
        if (el.disabled) node.disabled = true;
        out.push(node);
      }}
    }}
    for (const child of el.children) walk(child);
  }};
  walk(document.body);
  return {{
    url: location.href,
    title: document.title,
    viewport: {{ w: innerWidth, h: innerHeight }},
    total: out.length,
    truncated: out.length >= MAX,
    nodes: out,
  }};
}})()"#,
        max_nodes = max_nodes,
        query = escape(query)
    )
}

fn click_script(id: &str) -> String {
    format!(
        r#"(() => {{
  const el = {lookup};
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  const r = el.getBoundingClientRect();
  const opts = {{ bubbles: true, cancelable: true, composed: true, clientX: r.x + r.width / 2, clientY: r.y + r.height / 2, button: 0 }};
  for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click']) {{
    const Ctor = type.startsWith('pointer') && window.PointerEvent ? PointerEvent : MouseEvent;
    el.dispatchEvent(new Ctor(type, opts));
  }}
  return {{ clicked: el.getAttribute('data-prism-ui'), label: (el.innerText || el.value || '').trim().slice(0, 80) }};
}})()"#,
        lookup = lookup(id)
    )
}

fn fill_script(id: &str, value: &Value) -> String {
    let literal = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    format!(
        r#"(() => {{
  const el = {lookup};
  const value = {literal};
  if (el.type === 'checkbox' || el.type === 'radio') {{
    const want = value === true || value === 'true';
    if (el.checked !== want) el.click();
    return {{ id: el.getAttribute('data-prism-ui'), checked: el.checked }};
  }}
  const text = value === null || value === undefined ? '' : String(value);
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
  if (setter) setter.call(el, text); else el.value = text;
  el.dispatchEvent(new Event('input', {{ bubbles: true }}));
  el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ id: el.getAttribute('data-prism-ui'), value: el.value }};
}})()"#,
        lookup = lookup(id),
        literal = literal
    )
}

fn highlight_script(id: &str, ms: u64) -> String {
    format!(
        r#"(() => {{
  const el = {lookup};
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  const r = el.getBoundingClientRect();
  const box = document.createElement('div');
  box.setAttribute('data-prism-ui-highlight', '1');
  Object.assign(box.style, {{
    position: 'fixed', left: r.x - 3 + 'px', top: r.y - 3 + 'px',
    width: r.width + 6 + 'px', height: r.height + 6 + 'px',
    border: '2px solid #ff2fd0', borderRadius: '4px', pointerEvents: 'none',
    boxShadow: '0 0 0 9999px rgba(0,0,0,0.18)', zIndex: 2147483647,
    transition: 'opacity 200ms', opacity: '1',
  }});
  document.body.appendChild(box);
  setTimeout(() => {{ box.style.opacity = '0'; setTimeout(() => box.remove(), 250); }}, {ms});
  return {{ id: el.getAttribute('data-prism-ui'), ms: {ms} }};
}})()"#,
        lookup = lookup(id),
        ms = ms
    )
}

fn base64_decode(raw: &str) -> Result<Vec<u8>, String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in TABLE.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let mut out = Vec::with_capacity(raw.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in raw.bytes() {
        if byte == b'=' {
            break;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            continue; // 忽略换行等空白
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVs\nbG8=").unwrap(), b"hello");
    }

    #[test]
    fn snapshot_script_embeds_the_limit() {
        let script = snapshot_script(12, "play");
        assert!(script.contains("const MAX = 12;"));
        assert!(script.contains("\"play\""));
    }

    #[test]
    fn lookup_escapes_quotes() {
        let script = click_script("u1\"x");
        assert!(script.contains("u1\\\"x"));
    }
}
