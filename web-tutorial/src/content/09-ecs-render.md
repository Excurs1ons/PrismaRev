# 09 · ECS 驱动渲染（M3）

M2 画了一个静态网格。M3 让场景**活起来**：多个带 `Transform` 的实体被 `render_system` 每帧从 ECS World 查询，构建 `DrawItem` 列表，提交给 `GraphRenderer` 完成前向 PBR 渲染。这是引擎第一次「像一个引擎」在跑。

:::info 里程碑 M3 的目标
ECS 驱动的渲染：相机 + 每个实体的 Transform/MeshHandle/PbrMaterial → render_system 每帧查询世界、构建场景变更（SceneChanges）→ DirtyRouter 追踪差异 → GraphRenderer 消费 DrawItem 列表绘制。
:::

## 渲染组件

引擎的 ECS 组件定义在 `crates/prism-engine/src/render_system.rs`：

| 组件 | 字段 | 说明 |
|------|------|------|
| `Transform` | `translation`, `rotation`(四元数), `scale` | 实体局部到世界的变换，`to_model_matrix()` 生成列主序 `mat4` |
| `MeshHandle` | `usize` 索引 | 指向 `MeshManager` 里的 GPU 网格 |
| `PbrMaterial` | `albedo`, `metallic`, `roughness` | PBR 表面参数 |
| `RenderInstance` | `mesh`, `material_slot`, `model` | 可渲染对象的组合结构（从 glTF 实例创建） |
| `DirectionalLight` | `euler_xyz`, `intensity`, `color`, `ambient` | 方向光，角度+物理照度（lux） |
| `PointLight` | `position`, `range`, `color`, `intensity` | 点光源，物理发光强度（candela） |

```rust id=transform-comp
// Transform：从四元数生成模型矩阵
pub struct Transform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // (x, y, z, w) 四元数
    pub scale: [f32; 3],
}

impl Transform {
    pub fn to_model_matrix(&self) -> [[f32; 4]; 4] {
        // 列主序 mat4 [col][row] — 直接作为 GLSL mat4 使用
        // 四元数 → 旋转矩阵 + scale + translation
    }
}
```

:::tip 物理单位
引擎使用**物理单位**：光照强度用 `lux`（勒克斯，方向光）/ `candela`（坎德拉，点光源），而非无单位的魔数。这样不同场景的光照参数可以直接复用真实世界的参考值（晴天 100k lux，室内 100 cd）。
:::

## 相机

`OrbitCamera` 用球坐标（azimuth `theta`、elevation `phi`、距离 `distance`）围绕一个 `target` 旋转。它的 `view_proj()` 产出 `proj * view`（**列主序**，与 GLSL `m[col][row]` 对齐）：

```rust id=camera-vp
pub fn view_proj(&self) -> [[f32; 4]; 4] {
    let proj = self.perspective();
    let view = self.look_at(self.eye());
    proj * view   // 列主序矩阵乘法
}
```

:::danger 透视投影的 Vulkan y-flip
`perspective()` 里 `p[1][1] = -inv_tan(fovy/2)`（注意负号）。这是 Vulkan 与 OpenGL 的关键差异——OpenGL 用 `+inv_tan`。深度映射到 `[0,1]` 而非 `[-1,1]`。漏掉这个负号，画面会上下颠倒。详见第 13 章坐标约定。
:::

## 渲染管线：数据流全景

每帧的渲染流程不再是一条 `render_system` 函数到底，而是四个阶段：

```
ECS World
    │
    ▼
collect_scene_changes()  ← PR-S1：从 ECS 抽离场景快照
    │ 产出 SceneChanges { view_proj, eye, view, draw_list, ... }
    ▼
DirtyRouter::update()    ← PR-S2：对比上一帧，产出 DirtyFlags
    │ 标记 camera/directional_light/point_lights 是否变更
    ▼
FrameInput 构建           ← GraphRenderer::begin_frame
    │ 接收 FrameUBOData + DrawItem 列表 + GTAO inputs
    ▼
GraphRenderer::render()  ← begin_frame → ScenePass/Gtao/Post → present
    │ 遍历 draw_list，逐物体提交绘制调用
    ▼
屏幕
```

### collect_scene_changes：ECS → 场景快照

每帧开头，`collect_scene_changes` 从 `World` 中查询所有相关组件，构建一个纯数据结构的 `SceneChanges`。

```rust
pub fn collect_scene_changes(
    world: &World, camera: &Camera, mesh_manager: &MeshManager,
) -> SceneChanges {
    // 查询 Transform + RenderInstance 实体 → DrawItem 列表
    for (entity, tf, inst) in world.query2::<Transform, RenderInstance>() {
        let model = tf.to_model_matrix();
        draw_list.push(DrawItem { mesh: inst.mesh, model, material_slot: inst.material_slot });
    }
    // 查询 DirectionalLight → 提取方向/颜色/照度
    // 查询 PointLight → 提取位置/范围/颜色/强度
    // 计算 view_proj, eye, view
    SceneChanges { view_proj, eye, view, draw_list, directional_light, point_lights, ... }
}
```

### DirtyRouter：帧间差异追踪

`DirtyRouter` 保存上一帧的 `SceneChanges`，对比当前帧后产出 `DirtyFlags` 标志位集合：

| 标志 | 含义 |
|------|------|
| `camera` | view-proj、eye、view 任意变化 |
| `directional_light` | 方向光参数变化 |
| `point_lights` | 点光源列表/参数变化 |

这些标志用于跳过冗余的 GPU 上传（如点光源未变则不重写 SSBO）。

### GraphRenderer 消费

```rust
// render_system 的最终调用
graph_renderer.render(
    &FrameInput {
        ubo: frame_ubo_data,
        draw_list: scene_changes.draw_list,
        gtao_inputs,  // ScenePass → GtaoPass 的 AO 数据
        ...
    },
);
```

`GraphRenderer` 内部驱动 `ShadowMapPass → ScenePass → GtaoPass → PostPass` 链（见第 7 章）。每帧遍历 `draw_list`，为每个 `DrawItem` 计算 model 矩阵、绑定点光源、录制绘制命令。

:::tip 系统即函数，世界即数据
注意 `render_system` 是**普通函数**，不是某个「渲染器对象」的方法。它与 `World` 解耦：换一套逻辑只需换一个系统函数。这是 ECS 相比 OOP 的核心优势——逻辑可组合、可测试、无继承耦合。
:::

## 交互演示：坐标变换

下方可视化展示一个立方体从**世界空间**（右手系）经 `clip = P·V·M` 变换到 **NDC**。拖拽旋转相机，观察 Vulkan 下 NDC 的 **y 轴朝下**（y=+1 在底部）、深度 **z ∈ [0,1]**。点「切换 y-flip」可对比 OpenGL 约定：

（在页面下方查看交互演示）

:::exercise
1. 在 `crates/prism-engine/src/render_system.rs` 里找到 `collect_scene_changes` 函数，列出它从 World 查询了哪几种组件组合。
2. 在场景里 spawn 一个带 `Transform` 但没有 `RenderInstance` 的实体，验证渲染系统会**忽略**它（因为 `query2::<Transform, RenderInstance>` 不匹配）。
3. 打开 `camera.rs`，把 `perspective()` 里的负号去掉，运行看画面如何颠倒——亲手验证 y-flip 的必要性。
4. 跟踪一个 `DirtyFlags::camera = true` 到 `set_ubo_data` 的数据流：哪些 GPU 资源会被重写？
:::

下一章，我们让场景不再手写——从 glTF 文件加载真实资产。
