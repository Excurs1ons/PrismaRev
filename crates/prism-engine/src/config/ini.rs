//! 项目配置文件（`assets/settings.ini`）的 INI 方言解析器。
//!
//! 方言（与 `docs/DESIGN.md` §「项目配置文件」一致，改动要同步文档）：
//!
//! 1. `[section]` 行，节内 `key = value`；节与节之间顺序无关；
//! 2. 整行注释：`#` 或 `;` 开头（前导空白忽略）；**行内不解析注释**
//!    ——磁盘路径里的 `#` / `;` 因此不会截断值；
//! 3. 值可加双引号，引号内支持 `\"` `\\` `\n`；不加引号时取到行尾并 trim；
//! 4. 解析器只产出 `String`；类型转换（u32 / bool / f32）由调用方做，
//!    失败时用默认值 + `log::warn!`（见 [`Ini::get_u32`] 等）；
//! 5. 未知 section / 未知 key 由调用方忽略（本模块不预设 schema）；
//! 6. 同一 section 出现两次：**合并**，同名 key 后者覆盖前者。
//!
//! 这里刻意不引入 `toml` / `serde`：配置只是几十个标量字段，自写解析器让
//! 运行时依赖树里少掉 `toml_parser` / `winnow` / `toml_datetime`（约 60–80 KB）。

use std::fmt::Write as _;

/// 一个 `[section]`：名字 + 有序的 `key = value`（值已解引号，仍是字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IniSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl IniSection {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// 写入一个键：已存在则原地覆盖（保持原有顺序），否则追加到末尾。
    fn set(&mut self, key: &str, value: &str) {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
        {
            *slot = value.to_string();
        } else {
            self.entries.push((key.to_string(), value.to_string()));
        }
    }
}

/// 一份解析后的配置文件：section 顺序即文件顺序，重复 section 已合并。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ini {
    sections: Vec<IniSection>,
}

impl Ini {
    /// 解析文本。任何畸形行都只影响它自己（跳过 + `log::debug!`），不会让
    /// 整份配置失效——配置文件坏了也要能启动，只是回到默认值。
    pub fn parse(text: &str) -> Self {
        let mut ini = Self::default();
        // 当前 section 的下标；`None` = 还没见到 `[section]`（节前键被忽略）。
        let mut current: Option<usize> = None;

        for raw in text.lines() {
            let line = raw.trim_end_matches('\r').trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if let Some(name) = section_header(line) {
                if name.is_empty() {
                    log::debug!("ini: empty section header on {raw:?}; ignored");
                    continue;
                }
                // 重复 section 合并：找到同名节就继续往它里面写。
                current = Some(match ini.sections.iter().position(|s| s.name == name) {
                    Some(index) => index,
                    None => {
                        ini.sections.push(IniSection {
                            name: name.to_string(),
                            entries: Vec::new(),
                        });
                        ini.sections.len() - 1
                    }
                });
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                log::debug!("ini: {raw:?} is neither a section nor `key = value`; ignored");
                continue;
            };
            let key = key.trim();
            if key.is_empty() {
                log::debug!("ini: {raw:?} has an empty key; ignored");
                continue;
            }
            let Some(index) = current else {
                log::debug!("ini: {raw:?} appears before any [section]; ignored");
                continue;
            };
            ini.sections[index].set(key, &parse_value(value));
        }

        ini
    }

    pub fn sections(&self) -> &[IniSection] {
        &self.sections
    }

    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|section| section.name == name)
    }

    /// 原始字符串值（已解引号）。缺失时 `None`。
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.get(key)
    }

    pub fn get_u32(&self, section: &str, key: &str) -> Option<u32> {
        let raw = self.get(section, key)?;
        match raw.trim().parse::<u32>() {
            Ok(value) => Some(value),
            Err(_) => {
                log::warn!("settings: [{section}] {key} = {raw:?} is not a u32; using default");
                None
            }
        }
    }

    pub fn get_i32(&self, section: &str, key: &str) -> Option<i32> {
        let raw = self.get(section, key)?;
        match raw.trim().parse::<i32>() {
            Ok(value) => Some(value),
            Err(_) => {
                log::warn!("settings: [{section}] {key} = {raw:?} is not an i32; using default");
                None
            }
        }
    }

    pub fn get_f32(&self, section: &str, key: &str) -> Option<f32> {
        let raw = self.get(section, key)?;
        match raw.trim().parse::<f32>() {
            Ok(value) if value.is_finite() => Some(value),
            _ => {
                log::warn!("settings: [{section}] {key} = {raw:?} is not a number; using default");
                None
            }
        }
    }

    /// `true` / `false`（大小写无关），另接受 `1` / `0` / `yes` / `no` / `on` / `off`。
    pub fn get_bool(&self, section: &str, key: &str) -> Option<bool> {
        let raw = self.get(section, key)?;
        match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => {
                log::warn!("settings: [{section}] {key} = {raw:?} is not a bool; using default");
                None
            }
        }
    }

    /// 写入一个键（节不存在时创建，追加到文件末尾）。
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let index = match self.sections.iter().position(|s| s.name == section) {
            Some(index) => index,
            None => {
                self.sections.push(IniSection {
                    name: section.to_string(),
                    entries: Vec::new(),
                });
                self.sections.len() - 1
            }
        };
        self.sections[index].set(key, value);
    }

    /// 规范化为文本：section 顺序 = 插入顺序，键顺序 = 写入顺序。
    ///
    /// 只做「值 → 文本」这一步，不排序也不丢字段，因此写回时未知段/未知键
    /// 能原样保留（注释除外——INI 没有可往返的注释模型）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (index, section) in self.sections.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "[{}]", section.name);
            for (key, value) in &section.entries {
                let _ = writeln!(out, "{} = {}", key, format_value(value));
            }
        }
        out
    }
}

fn section_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.trim())
}

/// 解引号：`"..."` 里支持 `\"` `\\` `\n`；不加引号时取到行尾并 trim。
///
/// 未知转义（例如 Windows 路径里的 `C:\foo`）保留反斜杠本体，不吞字符。
pub fn parse_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(body) = trimmed.strip_prefix('"') else {
        return trimmed.to_string();
    };

    let mut out = String::new();
    let mut chars = body.char_indices();
    let mut closed_at = None;
    while let Some((offset, ch)) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, '"')) => out.push('"'),
                Some((_, '\\')) => out.push('\\'),
                Some((_, other)) => {
                    // 未知转义：原样保留，避免把 `C:\temp` 吃掉一个字符。
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '"' => {
                closed_at = Some(offset);
                break;
            }
            other => out.push(other),
        }
    }

    match closed_at {
        Some(offset) => {
            let rest = body[offset + 1..].trim();
            if !rest.is_empty() {
                log::debug!("ini: ignoring {rest:?} after the closing quote of {trimmed:?}");
            }
        }
        None => log::debug!("ini: unterminated quoted value {trimmed:?}; using it verbatim"),
    }
    out
}

/// 写回时的值格式：数字 / 布尔裸写（`width = 1600`、`vsync = false`），
/// 其余字符串加引号（`company = "Acme"`）；需要转义或含首尾空白时也加引号。
///
/// 「裸写数字看起来像数字」是安全的：解析器只产出字符串，`"1234"` 与 `1234`
/// 读回来完全一样。
pub fn format_value(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value != value.trim()
        || value.contains('"')
        || value.contains('\\')
        || value.contains('\n')
        || value.contains('\r');
    if !needs_quotes {
        if looks_like_scalar(value) {
            return value.to_string();
        }
        return format!("\"{value}\"");
    }

    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn looks_like_scalar(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off"
    ) || value.parse::<i64>().is_ok()
        || value.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_keys() {
        let ini = Ini::parse("[window]\nwidth = 1600\nheight = 900\n\n[app]\ncompany = \"Acme\"\n");
        assert_eq!(ini.get("window", "width"), Some("1600"));
        assert_eq!(ini.get("window", "height"), Some("900"));
        assert_eq!(ini.get("app", "company"), Some("Acme"));
        assert_eq!(ini.get("app", "missing"), None);
        assert_eq!(ini.get("nope", "width"), None);
    }

    #[test]
    fn both_comment_styles_are_ignored_but_inline_hashes_are_values() {
        let ini = Ini::parse(
            "# a comment\n   ; indented comment\n\n[build]\noutput_dir = \"a#b;c\" # not a comment\n",
        );
        assert_eq!(ini.sections().len(), 1);
        // 行内不解析注释：引号里的 `#` / `;` 原样保留。
        assert_eq!(ini.get("build", "output_dir"), Some("a#b;c"));
    }

    #[test]
    fn inline_comment_markers_are_plain_value_characters() {
        // 不加引号时取到行尾：`#` / `;` 不会被当成注释起点。
        let ini = Ini::parse("[build]\noutput_dir = build/win#1;x\n");
        assert_eq!(ini.get("build", "output_dir"), Some("build/win#1;x"));
    }

    #[test]
    fn comments_before_any_section_do_not_create_one() {
        let ini = Ini::parse("; header\n[render]\nrender_mode = raster\n");
        assert_eq!(ini.sections().len(), 1);
        assert_eq!(ini.sections()[0].name, "render");
    }

    #[test]
    fn quoted_values_unescape_and_unquoted_values_are_trimmed() {
        let ini = Ini::parse(
            "[app]\nname = \"say \\\"hi\\\"\"\npath = \"C:\\\\temp\\\\x\"\nline = \"a\\nb\"\nraw =   padded value   \n",
        );
        assert_eq!(ini.get("app", "name"), Some("say \"hi\""));
        assert_eq!(ini.get("app", "path"), Some("C:\\temp\\x"));
        assert_eq!(ini.get("app", "line"), Some("a\nb"));
        assert_eq!(ini.get("app", "raw"), Some("padded value"));
    }

    #[test]
    fn unknown_escape_keeps_the_backslash() {
        assert_eq!(parse_value("\"C:\\temp\""), "C:\\temp");
    }

    #[test]
    fn duplicate_sections_merge_and_last_key_wins() {
        let ini =
            Ini::parse("[app]\nname = \"one\"\n[window]\nwidth = 800\n[app]\nname = \"two\"\n");
        assert_eq!(ini.sections().len(), 2);
        assert_eq!(ini.sections()[0].entries.len(), 1);
        assert_eq!(ini.get("app", "name"), Some("two"));
        assert_eq!(ini.get("window", "width"), Some("800"));
    }

    #[test]
    fn empty_values_and_empty_quotes() {
        let ini = Ini::parse("[app]\nname =\ntitle = \"\"\n");
        assert_eq!(ini.get("app", "name"), Some(""));
        assert_eq!(ini.get("app", "title"), Some(""));
    }

    #[test]
    fn crlf_line_endings_are_supported() {
        let ini = Ini::parse("[window]\r\nwidth = 1280\r\ntitle = \"Win\"\r\n");
        assert_eq!(ini.get("window", "width"), Some("1280"));
        assert_eq!(ini.get("window", "title"), Some("Win"));
    }

    #[test]
    fn malformed_lines_are_skipped_without_losing_the_rest() {
        let ini = Ini::parse("stray = 1\n[window\nwidth = 640\n= 3\n[t]\nok = 1\n");
        // `[window` 没有闭合，被当成畸形行；`width` 仍属于节前（无 section）而丢弃。
        assert_eq!(ini.get("t", "ok"), Some("1"));
        assert_eq!(ini.get("window", "width"), None);
    }

    #[test]
    fn typed_getters_convert_and_warn_on_bad_values() {
        let ini =
            Ini::parse("[window]\nwidth = 1280\nvsync = TRUE\nratio = 1.5\nbad = nope\nneg = -3\n");
        assert_eq!(ini.get_u32("window", "width"), Some(1280));
        assert_eq!(ini.get_bool("window", "vsync"), Some(true));
        assert_eq!(ini.get_f32("window", "ratio"), Some(1.5));
        assert_eq!(ini.get_u32("window", "bad"), None);
        assert_eq!(ini.get_i32("window", "neg"), Some(-3));
    }

    #[test]
    fn set_creates_sections_and_renders_round_trip() {
        let mut ini = Ini::parse("# note\n[app]\nname = \"Acme\"\n");
        ini.set("app", "version", "1.2.3");
        ini.set("window", "width", "1024");
        let text = ini.render();
        let reparsed = Ini::parse(&text);
        assert_eq!(reparsed.get("app", "name"), Some("Acme"));
        assert_eq!(reparsed.get("app", "version"), Some("1.2.3"));
        assert_eq!(reparsed.get("window", "width"), Some("1024"));
        assert!(!text.contains("# note"), "注释不参与往返");
    }

    #[test]
    fn render_quotes_only_when_needed() {
        assert_eq!(format_value("1600"), "1600");
        assert_eq!(format_value("false"), "false");
        assert_eq!(format_value("1.5"), "1.5");
        assert_eq!(format_value("Acme"), "\"Acme\"");
        assert_eq!(format_value(""), "\"\"");
        assert_eq!(format_value(" padded "), "\" padded \"");
        assert_eq!(format_value("a\"b"), "\"a\\\"b\"");
        assert_eq!(format_value("a\\b"), "\"a\\\\b\"");
        assert_eq!(format_value("a\nb"), "\"a\\nb\"");
        assert_eq!(
            Ini::parse(&format!("[x]\nk = {}\n", format_value("a\"b"))).get("x", "k"),
            Some("a\"b")
        );
    }
}
