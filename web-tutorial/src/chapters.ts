// 章节清单：每章的 id（用于 URL hash 路由）、标题、副标题、内容文件。
// 新增/调整章节只需改这里，侧边栏与路由会自动更新。

export interface ChapterMeta {
  id: string;
  title: string;
  subtitle: string;
  file: string; // 对应 src/content 下的 .md 文件名
  // 侧边栏分组
  group: "基础" | "图形" | "引擎";
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
    title: "03 · 引入第三方库",
    subtitle: "Cargo.toml 依赖管理与 workspace",
    file: "03-deps.md",
    group: "基础",
  },
  {
    id: "winit",
    title: "04 · winit 窗口与事件循环",
    subtitle: "ApplicationHandler 与窗口生命周期",
    file: "04-winit.md",
    group: "基础",
  },
  {
    id: "context",
    title: "05 · ash + Vulkan 上下文",
    subtitle: "Instance / 设备 / 队列",
    file: "05-context.md",
    group: "图形",
    viz: ["frameLoop"],
  },
  {
    id: "swapchain",
    title: "06 · Swapchain 与清屏循环",
    subtitle: "M1：acquire→record→submit→present",
    file: "06-swapchain.md",
    group: "图形",
    viz: ["frameLoop"],
  },
  {
    id: "pipeline",
    title: "07 · Render Pass 与图形管线",
    subtitle: "M2：深度缓冲与第一个 mesh",
    file: "07-pipeline.md",
    group: "图形",
    viz: ["pipeline"],
  },
  {
    id: "ecs",
    title: "08 · ECS 内核设计",
    subtitle: "Entity / Component / World / Query",
    file: "08-ecs.md",
    group: "引擎",
    viz: ["ecsFlow", "memory"],
  },
  {
    id: "ecs-render",
    title: "09 · ECS 驱动渲染",
    subtitle: "M3：相机、Transform 与 Blinn-Phong",
    file: "09-ecs-render.md",
    group: "引擎",
    viz: ["coordSpace", "coordchain", "pipeline"],
  },
  {
    id: "assets",
    title: "10 · 资产管线",
    subtitle: "glTF 加载、纹理与 image crate",
    file: "10-assets.md",
    group: "引擎",
  },
  {
    id: "pbr",
    title: "11 · PBR + IBL 进阶",
    subtitle: "HDR 环境、bindless 与 debug view",
    file: "11-pbr.md",
    group: "引擎",
  },
  {
    id: "android",
    title: "12 · Android 移植",
    subtitle: "M4：android-activity 与 APK 打包",
    file: "12-android.md",
    group: "引擎",
    viz: ["deployFlow"],
  },
  {
    id: "review",
    title: "13 · 引擎架构复盘",
    subtitle: "数据流、crate 职责与坐标约定",
    file: "13-review.md",
    group: "引擎",
    viz: ["coordSpace", "coordchain", "rendergraph"],
  },
];

export function findChapter(id: string): ChapterMeta | undefined {
  return CHAPTERS.find((c) => c.id === id);
}
