// 章节清单——按开发依赖顺序排列：先零依赖子系统，后图形，最后整合。
// 每个子系统群组对应明确的 crate 边界。

export interface ChapterMeta {
  id: string;
  title: string;
  subtitle: string;
  file: string; // 对应 src/content 下的 .md 文件名
  // 侧边栏分组（代表子系统层次）
  group: "基础" | "数据层" | "图形层" | "渲染层" | "引擎层" | "平台与回顾";
  // 该章嵌入的交互可视化组件 key（可选，可多个）
  viz?: VizKey[];
}

export type VizKey =
  | "frameLoop"
  | "ecsFlow"
  | "coordSpace"
  | "deployFlow"
  | "memory"
  | "pipeline"
  | "rendergraph"
  | "coordchain";

export const CHAPTERS: ChapterMeta[] = [
  // ========== 基础篇 ==========
  {
    id: "intro",
    title: "01 · 导言",
    subtitle: "引擎概览、学习路线与环境搭建",
    file: "01-intro.md",
    group: "基础",
  },
  {
    id: "hello",
    title: "02 · Rust Hello World",
    subtitle: "Cargo 初识、main、编译运行",
    file: "02-hello.md",
    group: "基础",
  },
  {
    id: "deps",
    title: "03 · Cargo 与工作区",
    subtitle: "依赖管理与 workspace 布局",
    file: "03-deps.md",
    group: "基础",
  },

  // ========== 数据层 (prism-ecs + prism-asset) ==========
  // 零 workspace 依赖的纯数据子系统，可独立开发测试
  {
    id: "ecs",
    title: "04 · ECS 内核设计",
    subtitle: "Entity / Component / World / Query",
    file: "08-ecs.md",
    group: "数据层",
    viz: ["ecsFlow", "memory"],
  },
  {
    id: "assets",
    title: "05 · 资产管线",
    subtitle: "glTF 加载、纹理与 prism-asset 架构",
    file: "10-assets.md",
    group: "数据层",
  },

  // ========== 图形层 (prism-render: 窗口 + GPU) ==========
  {
    id: "winit",
    title: "06 · winit 窗口系统",
    subtitle: "ApplicationHandler 与窗口生命周期",
    file: "04-winit.md",
    group: "图形层",
  },
  {
    id: "context",
    title: "07 · Vulkan 上下文",
    subtitle: "Instance / 设备 / 队列",
    file: "05-context.md",
    group: "图形层",
    viz: ["frameLoop"],
  },
  {
    id: "swapchain",
    title: "08 · Swapchain 与帧循环",
    subtitle: "M1：acquire→record→submit→present",
    file: "06-swapchain.md",
    group: "图形层",
    viz: ["frameLoop"],
  },
  {
    id: "pipeline",
    title: "09 · 着色器与渲染管线",
    subtitle: "M2：深度缓冲与第一个 mesh",
    file: "07-pipeline.md",
    group: "图形层",
    viz: ["pipeline"],
  },

  // ========== 渲染层 (光照 + 后处理) ==========
  {
    id: "pbr",
    title: "10 · PBR：从纯色到物理渲染",
    subtitle: "六步渐进、IBL、Bindless",
    file: "11-pbr.md",
    group: "渲染层",
  },
  {
    id: "render-advanced",
    title: "11 · 后处理与高级渲染",
    subtitle: "ReSTIR DI、路径追踪、SH GI",
    file: "14-render-advanced.md",
    group: "渲染层",
    viz: ["rendergraph"],
  },

  // ========== 引擎层 (prism-engine: ECS + 渲染 + 调试) ==========
  {
    id: "ecs-render",
    title: "12 · ECS 驱动渲染",
    subtitle: "M3：相机、Transform 与 Blinn-Phong",
    file: "09-ecs-render.md",
    group: "引擎层",
    viz: ["coordSpace", "coordchain", "pipeline"],
  },
  {
    id: "engine-tools",
    title: "13 · 引擎框架与调试工具",
    subtitle: "App 循环、AudioEngine、Inspector",
    file: "15-engine-tools.md",
    group: "引擎层",
  },

  // ========== 平台与回顾 ==========
  {
    id: "android",
    title: "14 · Android 移植",
    subtitle: "M4：android-activity 与 APK 打包",
    file: "12-android.md",
    group: "平台与回顾",
    viz: ["deployFlow"],
  },
  {
    id: "review",
    title: "15 · 架构复盘",
    subtitle: "数据流、crate 职责与坐标约定",
    file: "13-review.md",
    group: "平台与回顾",
    viz: ["coordSpace", "coordchain", "rendergraph"],
  },
];

export function findChapter(id: string): ChapterMeta | undefined {
  return CHAPTERS.find((c) => c.id === id);
}
