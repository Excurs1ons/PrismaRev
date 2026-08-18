# PrismaRev Sponza Path Tracing

这是与 `projects/game/` 同属用户项目目录的独立 workspace，演示引擎已有的实时路径追踪管线。
项目代码只依赖 `prism-app` 和 `prism-engine`，不直接访问 Vulkan、winit 或 ECS 内部实现。

## 运行

Sponza 资源需要先导入并构建到运行目录的资源包，且 manifest 中 `sponza` 应是首个可加载场景：

```powershell
cargo run --manifest-path projects/sponza/Cargo.toml
```

路径追踪由 `build_app()` 的 `RenderSettings::render_mode = PathTrace` 自动启用；需要支持
`VK_KHR_ray_query`、加速结构和相应 bindless 特性的 Vulkan 设备。能力不足时由引擎能力层降级或报告初始化错误。
