//! 设置类面板：项目设置 / 首选项 / 主题 / 构建 / 离线工具 / 快捷键 / 关于。
//!
//! 与 Web 编辑器一样，这些面板都是 **dock 面板**（不是 modal），默认并进
//! 检查器 leaf 的 tab，也可以拖出来浮动。

use gpui::{div, prelude::*, px, AnyElement, Context, Div, SharedString, Stateful};
use prism_editor_gpui_core::{Locale, ThemeToken, THEME_PRESETS};

use crate::app::{EditorView, TextTarget};
use crate::panels::{header_row, muted};
use crate::style::ROW_HEIGHT;

impl EditorView {
    pub fn project_settings_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header = header_row(style, self.workspace.t("menu.projectSettings"));
        let mut body = self.settings_scroll("project-settings", cx);

        body = body.child(section_title(style, "应用"));
        body = body.child(self.text_row("公司", self.project.app.company.clone(), "company", cx));
        body = body.child(self.text_row("产品名", self.project.app.name.clone(), "name", cx));
        body = body.child(self.text_row("版本", self.project.app.version.clone(), "version", cx));

        body = body.child(section_title(style, "窗口"));
        body = body.child(self.text_row("标题", self.project.window.title.clone(), "title", cx));
        body = body.child(self.number_row(
            "宽度",
            self.project.window.width as f32,
            16.0,
            320.0,
            7680.0,
            "proj-width",
            cx,
            |view, value| view.project.window.width = value.round().max(1.0) as u32,
        ));
        body = body.child(self.number_row(
            "高度",
            self.project.window.height as f32,
            16.0,
            240.0,
            4320.0,
            "proj-height",
            cx,
            |view, value| view.project.window.height = value.round().max(1.0) as u32,
        ));
        body = body.child(self.bool_row(
            "可调整大小",
            self.project.window.resizable,
            "proj-resizable",
            cx,
            |view, value| view.project.window.resizable = value,
        ));
        body = body.child(self.bool_row(
            "全屏",
            self.project.window.fullscreen,
            "proj-fullscreen",
            cx,
            |view, value| view.project.window.fullscreen = value,
        ));
        body = body.child(self.bool_row(
            "垂直同步",
            self.project.window.vsync,
            "proj-vsync",
            cx,
            |view, value| view.project.window.vsync = value,
        ));

        body = body.child(section_title(style, "构建"));
        body = body.child(self.text_row(
            "目标平台",
            self.project.build.target.clone(),
            "build_target",
            cx,
        ));
        body = body.child(self.text_row(
            "Profile",
            self.project.build.profile.clone(),
            "build_profile",
            cx,
        ));
        body = body.child(self.text_row(
            "输出目录",
            self.project.build.output_dir.clone(),
            "output_dir",
            cx,
        ));
        body = body.child(self.bool_row(
            "构建后运行",
            self.project.build.run_after,
            "proj-run-after",
            cx,
            |view, value| view.project.build.run_after = value,
        ));

        body = body.child(self.action_button(
            "settings-apply",
            "应用设置",
            ThemeToken::Success,
            cx,
            |this| match (this.workspace.project_root.clone(), &this.project) {
                (Some(root), settings) => match settings.write(&root) {
                    Ok(()) => this.workspace.log(
                        prism_editor_gpui_core::LogSource::System,
                        prism_editor_gpui_core::LogLevel::Info,
                        "已写入 assets/settings.toml（保留其它段）",
                    ),
                    Err(error) => this.workspace.log(
                        prism_editor_gpui_core::LogSource::System,
                        prism_editor_gpui_core::LogLevel::Error,
                        format!("写入 settings.toml 失败：{error}"),
                    ),
                },
                (None, _) => this.workspace.log(
                    prism_editor_gpui_core::LogSource::System,
                    prism_editor_gpui_core::LogLevel::Warn,
                    "没有绑定项目根，无法写入 settings.toml",
                ),
            },
        ));
        body = body.child(
            div()
                .px_2()
                .text_size(px(9.0))
                .text_color(style.muted())
                .child(SharedString::from(
                    "写回 assets/settings.toml 的 [app] / [window] / [build] 段，[render] 等其它段原样保留。",
                )),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn preferences_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header = header_row(style, self.workspace.t("menu.preferences"));
        let mut body = self.settings_scroll("preferences-body", cx);

        body = body.child(section_title(style, "编辑"));
        body = body.child(self.bool_row(
            "自动保存",
            self.preferences.autosave,
            "pref-autosave",
            cx,
            |view, value| view.preferences.autosave = value,
        ));
        body = body.child(self.bool_row(
            "删除前确认",
            self.preferences.confirm_delete,
            "pref-confirm",
            cx,
            |view, value| view.preferences.confirm_delete = value,
        ));
        body = body.child(self.bool_row(
            "紧凑模式",
            self.preferences.compact,
            "pref-compact",
            cx,
            |view, value| view.preferences.compact = value,
        ));
        body = body.child(self.text_row(
            "默认运行场景",
            self.preferences.default_scene.clone(),
            "default_scene",
            cx,
        ));

        body = body.child(section_title(style, "日志"));
        body = body.child(self.number_row(
            "最大行数",
            self.preferences.max_log_lines,
            500.0,
            500.0,
            20_000.0,
            "pref-log-lines",
            cx,
            |view, value| {
                view.preferences.max_log_lines = value;
                view.workspace.log.set_capacity(value as usize);
            },
        ));
        body = body.child(div().text_size(px(9.0)).text_color(style.muted()).child(
            SharedString::from(
                "日志面板是窗口化渲染：这个值只影响保留的历史行数，不影响 DOM 节点数。",
            ),
        ));

        body = body.child(section_title(style, "语言"));
        body = body.child(self.bool_row(
            "跟随系统语言",
            self.preferences.locale_auto,
            "pref-locale-auto",
            cx,
            |view, value| view.preferences.locale_auto = value,
        ));
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(ROW_HEIGHT)
                .px_2()
                .child(
                    div()
                        .w(px(120.0))
                        .text_size(px(10.0))
                        .text_color(style.text_soft())
                        .child(SharedString::from("界面语言")),
                )
                .children([Locale::ZhCn, Locale::EnUs].into_iter().map(|locale| {
                    let active = self.workspace.i18n.locale() == locale;
                    div()
                        .id(SharedString::from(format!("locale-{}", locale.code())))
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(if active {
                            style.accent()
                        } else {
                            style.border()
                        })
                        .text_size(px(10.0))
                        .text_color(if active {
                            style.text_strong()
                        } else {
                            style.text_soft()
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preferences.locale_auto = false;
                            this.workspace.set_locale(locale);
                            cx.notify();
                        }))
                        .child(SharedString::from(locale.label()))
                })),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn theme_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header =
            header_row(style, self.workspace.t("menu.theme"))
                .justify_between()
                .child(div().text_size(px(9.0)).text_color(style.muted()).child(
                    SharedString::from(format!(
                        "预设：{} · 覆盖：{} 项",
                        self.workspace.theme.preset,
                        self.workspace.theme.overrides.len()
                    )),
                ));
        let mut body = self.settings_scroll("theme-body", cx);

        body = body.child(section_title(style, "预设"));
        let mut presets = div().flex().flex_row().flex_wrap().gap_1().px_2().py_1();
        for preset in THEME_PRESETS {
            let active = self.workspace.theme.preset == preset.id;
            let colors = preset.colors;
            presets = presets.child(
                div()
                    .id(SharedString::from(format!("theme-{}", preset.id)))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1()
                    .w(px(120.0))
                    .rounded_sm()
                    .border_1()
                    .border_color(if active {
                        style.accent()
                    } else {
                        style.border()
                    })
                    .bg(style.panel_alt())
                    .cursor_pointer()
                    .hover(move |this| this.bg(style.row_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.workspace.set_theme_preset(preset.id);
                        this.theme_changed();
                        cx.notify();
                    }))
                    .child(
                        div().flex().flex_row().gap_0p5().children(
                            [
                                ThemeToken::Bg,
                                ThemeToken::Panel,
                                ThemeToken::Accent,
                                ThemeToken::Success,
                                ThemeToken::Danger,
                            ]
                            .into_iter()
                            .map(|token| {
                                let color = colors.color(token);
                                div()
                                    .w(px(16.0))
                                    .h(px(10.0))
                                    .rounded_sm()
                                    .bg(crate::style::hsla(color))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(if active {
                                style.text_strong()
                            } else {
                                style.text()
                            })
                            .child(SharedString::from(preset.label)),
                    )
                    .child(
                        div()
                            .text_size(px(8.0))
                            .text_color(style.muted())
                            .child(SharedString::from(preset.description)),
                    ),
            );
        }
        body = body.child(presets);

        body = body.child(section_title(style, "颜色 token"));
        for group in prism_editor_gpui_core::theme::TOKEN_GROUPS {
            body = body.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(9.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(style.text_soft())
                    .child(SharedString::from(group.label)),
            );
            let mut grid = div().flex().flex_row().flex_wrap().gap_1().px_2();
            for token in group.tokens {
                let color = self.workspace.color(*token);
                let overridden = self.workspace.theme.overrides.contains_key(token);
                let token = *token;
                grid = grid.child(
                    div()
                        .id(SharedString::from(format!("theme-token-{}", token.key())))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .w(px(150.0))
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(move |this| this.bg(style.row_hover()))
                        .text_size(px(9.0))
                        .text_color(style.text_soft())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.theme_edit = Some(token);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(12.0))
                                .h(px(12.0))
                                .rounded_sm()
                                .border_1()
                                .border_color(style.border_strong())
                                .bg(crate::style::hsla(color)),
                        )
                        .child(SharedString::from(token.label()))
                        .when(overridden, |this| {
                            this.child(div().text_color(style.accent()).child("•"))
                        }),
                );
            }
            body = body.child(grid);
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn build_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let mut header = header_row(style, self.workspace.t("menu.buildPanel")).justify_between();
        header =
            header
                .child(div().text_size(px(9.0)).text_color(style.muted()).child(
                    SharedString::from(format!(
                        "{} · {} · {}",
                        self.project.build.target,
                        self.project.build.profile,
                        self.project.build.output_dir
                    )),
                ))
                .child(
                    div()
                        .id("build-run")
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .border_1()
                        .border_color(style.success())
                        .text_size(px(9.0))
                        .text_color(style.success())
                        .cursor_pointer()
                        .hover(move |this| this.bg(style.row_hover()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.run_build();
                            cx.notify();
                        }))
                        .child("构建项目"),
                );

        let scenes: Vec<(String, String)> = self
            .workspace
            .docs
            .iter()
            .filter(|doc| doc.kind == prism_editor_gpui_core::DocKind::Scene)
            .map(|doc| {
                (
                    doc.title(),
                    doc.path.clone().unwrap_or_else(|| "（未保存）".to_string()),
                )
            })
            .collect();

        let mut body = self.settings_scroll("build-body", cx);
        body = body.child(section_title(style, "场景（顺序即构建顺序）"));
        if scenes.is_empty() {
            body = body.child(muted(style, "没有场景"));
        }
        for (index, (name, path)) in scenes.iter().enumerate() {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(ROW_HEIGHT)
                    .px_2()
                    .text_size(px(10.0))
                    .child(
                        div()
                            .w(px(18.0))
                            .text_color(style.muted())
                            .child(SharedString::from(format!("{}", index))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(style.text())
                            .child(SharedString::from(name.clone())),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(SharedString::from(path.clone())),
                    ),
            );
        }

        body = body.child(section_title(style, "构建日志"));
        if self.build_log.is_empty() {
            body = body.child(muted(style, "尚未构建"));
        }
        for line in &self.build_log {
            body = body.child(
                div()
                    .px_2()
                    .text_size(px(10.0))
                    .text_color(style.text_soft())
                    .child(SharedString::from(line.clone())),
            );
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn tools_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header = header_row(style, self.workspace.t("menu.tools"));
        let mut body = self.settings_scroll("tools-body", cx);

        body = body.child(section_title(
            style,
            "Bake GI（prism-build-pipeline bake-gi）",
        ));
        body = body.child(self.number_row(
            "每条探针光线",
            self.bake.rays,
            32.0,
            16.0,
            4096.0,
            "bake-rays",
            cx,
            |view, value| view.bake.rays = value,
        ));
        body = body.child(self.number_row(
            "反弹次数",
            self.bake.bounces,
            1.0,
            1.0,
            8.0,
            "bake-bounces",
            cx,
            |view, value| view.bake.bounces = value,
        ));
        body = body.child(self.text_row("输出", self.bake.output.clone(), "bake_output", cx));
        body = body.child(self.action_button(
            "bake-run",
            "执行 bake-gi",
            ThemeToken::Accent,
            cx,
            |this| {
                let command = format!(
                    "prism-build-pipeline bake-gi --rays {} --bounces {} --output {}",
                    this.bake.rays, this.bake.bounces, this.bake.output
                );
                this.workspace.log(
                    prism_editor_gpui_core::LogSource::System,
                    prism_editor_gpui_core::LogLevel::Info,
                    format!("运行：{command}"),
                );
            },
        ));

        body = body.child(section_title(
            style,
            "程序化高度图（prism-build-pipeline heightmap）",
        ));
        body = body.child(self.number_row(
            "宽度",
            self.heightmap.width,
            64.0,
            64.0,
            8192.0,
            "hm-width",
            cx,
            |view, value| view.heightmap.width = value,
        ));
        body = body.child(self.number_row(
            "高度",
            self.heightmap.height,
            64.0,
            64.0,
            8192.0,
            "hm-height",
            cx,
            |view, value| view.heightmap.height = value,
        ));
        body = body.child(self.number_row(
            "Seed",
            self.heightmap.seed,
            1.0,
            0.0,
            100_000.0,
            "hm-seed",
            cx,
            |view, value| view.heightmap.seed = value,
        ));
        body = body.child(self.number_row(
            "迭代",
            self.heightmap.iterations,
            1.0,
            1.0,
            32.0,
            "hm-iterations",
            cx,
            |view, value| view.heightmap.iterations = value,
        ));
        body = body.child(self.number_row(
            "海平面",
            self.heightmap.sea_level,
            0.05,
            0.0,
            1.0,
            "hm-sea",
            cx,
            |view, value| view.heightmap.sea_level = value,
        ));
        body = body.child(self.action_button(
            "heightmap-run",
            "执行 heightmap",
            ThemeToken::Success,
            cx,
            |this| {
                let command = format!(
                    "prism-build-pipeline heightmap --width {} --height {} --seed {} --iterations {} --sea-level {}",
                    this.heightmap.width,
                    this.heightmap.height,
                    this.heightmap.seed,
                    this.heightmap.iterations,
                    this.heightmap.sea_level
                );
                this.workspace.log(
                    prism_editor_gpui_core::LogSource::System,
                    prism_editor_gpui_core::LogLevel::Info,
                    format!("运行：{command}"),
                );
            },
        ));

        body = body.child(
            div()
                .px_2()
                .py_1()
                .text_size(px(9.0))
                .text_color(style.muted())
                .child(SharedString::from(
                    "这些命令在 Web 编辑器里以子进程运行（编辑器不链接 cook/build 流水线）；GPUI 壳同样是占位执行。",
                )),
        );

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn shortcuts_panel(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header = header_row(style, self.workspace.t("menu.shortcuts"));
        let mut body = self.settings_scroll("shortcuts-body", _cx);
        let mut entries: Vec<(String, &'static str)> = prism_editor_gpui_core::SHORTCUTS
            .iter()
            .map(|entry| (self.workspace.t(entry.command.label_key()), entry.keys))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (label, keys) in entries {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .h(ROW_HEIGHT)
                    .px_2()
                    .border_b_1()
                    .border_color(style.border())
                    .text_size(px(10.0))
                    .child(
                        div()
                            .text_color(style.text())
                            .child(SharedString::from(label)),
                    )
                    .child(
                        div()
                            .px_1()
                            .rounded_sm()
                            .bg(style.bg_sunken())
                            .text_color(style.text_soft())
                            .child(SharedString::from(keys)),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub fn about_panel(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        let style = self.style;
        let header = header_row(style, self.workspace.t("menu.about"));
        let body = div()
            .flex_1()
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_size(px(11.0))
            .text_color(style.text_soft())
            .child(
                div()
                    .text_size(px(16.0))
                    .text_color(style.text_strong())
                    .child("PrismaRev Editor · GPUI"),
            )
            .child(SharedString::from(env!("CARGO_PKG_VERSION")))
            .child(SharedString::from(
                "从零构建的高性能 Vulkan 跨平台游戏引擎编辑器。",
            ))
            .child(SharedString::from(
                "这一份是 GPUI 原生壳：dock / 层级 / 检查器 / 资产 / 日志 / 视口全部由 Rust 直接绘制，",
            ))
            .child(SharedString::from(
                "数据层复用 Web 编辑器的 schema 与 .scene.json 形状，两者可以读同一份 fixture。",
            ))
            .child(
                div()
                    .mt_2()
                    .text_size(px(9.0))
                    .text_color(style.muted())
                    .child(SharedString::from(concat!(
                        "gpui ",
                        "0.2 · editor-gpui-core ",
                        env!("CARGO_PKG_VERSION")
                    ))),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    // ── 共用小控件 ─────────────────────────────────────────────────────

    fn settings_scroll(&self, id: &'static str, _cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(id)
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py_1()
    }

    fn text_row(
        &mut self,
        label: &'static str,
        value: String,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = self.style;
        let value_for_edit = value.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(ROW_HEIGHT)
            .px_2()
            .child(
                div()
                    .w(px(120.0))
                    .text_size(px(10.0))
                    .text_color(style.text_soft())
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .id(SharedString::from(format!("setting-{key}")))
                    .flex_1()
                    .h(px(18.0))
                    .px_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(style.border())
                    .bg(style.bg_sunken())
                    .flex()
                    .items_center()
                    .cursor_text()
                    .text_size(px(10.0))
                    .text_color(style.text())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.begin_edit(
                            TextTarget::ProjectSettings(key),
                            value_for_edit.clone(),
                            cx,
                        );
                    }))
                    .child(SharedString::from(value)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn number_row(
        &mut self,
        label: &'static str,
        value: f32,
        step: f32,
        min: f32,
        max: f32,
        id: &'static str,
        cx: &mut Context<Self>,
        setter: fn(&mut Self, f32),
    ) -> AnyElement {
        let style = self.style;
        let text = if step >= 1.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.2}")
        };
        let dec_setter = setter;
        let inc_setter = setter;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(ROW_HEIGHT)
            .px_2()
            .child(
                div()
                    .w(px(120.0))
                    .text_size(px(10.0))
                    .text_color(style.text_soft())
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        step_button(style, format!("{id}-dec"), "−").on_click(cx.listener(
                            move |this, _, _, cx| {
                                let next = (value - step).clamp(min, max);
                                dec_setter(this, next);
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .text_center()
                            .text_size(px(10.0))
                            .text_color(style.text())
                            .child(SharedString::from(text)),
                    )
                    .child(
                        step_button(style, format!("{id}-inc"), "＋").on_click(cx.listener(
                            move |this, _, _, cx| {
                                let next = (value + step).clamp(min, max);
                                inc_setter(this, next);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn bool_row(
        &mut self,
        label: &'static str,
        checked: bool,
        id: &'static str,
        cx: &mut Context<Self>,
        setter: fn(&mut Self, bool),
    ) -> AnyElement {
        let style = self.style;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(ROW_HEIGHT)
            .px_2()
            .child(
                div()
                    .w(px(120.0))
                    .text_size(px(10.0))
                    .text_color(style.text_soft())
                    .child(SharedString::from(label)),
            )
            .child(
                div()
                    .id(id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        setter(this, !checked);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(12.0))
                            .h(px(12.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(if checked {
                                style.accent()
                            } else {
                                style.border_strong()
                            })
                            .when(checked, |this| this.bg(style.accent()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(8.0))
                            .text_color(style.color(ThemeToken::OnAccent))
                            .child(if checked { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .text_size(px(9.0))
                            .text_color(style.muted())
                            .child(if checked { "on" } else { "off" }),
                    ),
            )
            .into_any_element()
    }

    fn action_button(
        &mut self,
        id: &'static str,
        label: &'static str,
        token: ThemeToken,
        cx: &mut Context<Self>,
        action: fn(&mut Self),
    ) -> AnyElement {
        let style = self.style;
        div()
            .id(id)
            .m_2()
            .px_2()
            .py_1()
            .w(px(160.0))
            .rounded_sm()
            .border_1()
            .border_color(style.color(token))
            .text_size(px(10.0))
            .text_color(style.color(token))
            .cursor_pointer()
            .hover(move |this| this.bg(style.row_hover()))
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this);
                cx.notify();
            }))
            .child(SharedString::from(label))
            .into_any_element()
    }

    fn run_build(&mut self) {
        self.build_log.clear();
        self.building = true;
        let steps = [
            "cargo build --release（prisma）",
            "build-scenes（cook 场景包）",
            "收集 exe / scenes.pak / settings.toml",
        ];
        for step in steps {
            self.build_log.push(format!("✓ {step}"));
            self.workspace.log(
                prism_editor_gpui_core::LogSource::System,
                prism_editor_gpui_core::LogLevel::Info,
                step,
            );
        }
        self.build_log.push(format!(
            "完成 → {}（占位执行，未真正调用 cargo）",
            self.project.build.output_dir
        ));
        self.building = false;
    }
}

fn section_title(style: crate::style::Style, text: &'static str) -> Div {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_size(px(10.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(style.text_soft())
        .child(SharedString::from(text))
}

fn step_button(style: crate::style::Style, id: String, glyph: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(id))
        .w(px(18.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(style.border())
        .bg(style.panel_alt())
        .text_size(px(9.0))
        .text_color(style.text())
        .cursor_pointer()
        .hover(move |this| this.bg(style.row_hover()))
        .child(glyph)
}
