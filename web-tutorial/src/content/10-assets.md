# 10 · 资产管线

M3 的场景是代码里手写几个立方体。真实引擎要从**文件**加载——glTF 场景、PNG/JPEG 纹理、HDR 环境。这部分在 `prism-asset` crate。

:::info 里程碑 M5（资产管线）
目标：编译着色器、加载 glTF 2.0 场景、解码图像纹理，把「美术产出的资源」变成「GPU 上的缓冲与图像」。本章聚焦 glTF 与图像两条主线。
:::

## 为什么需要资产层

直接把顶点写死在代码里无法扩展。资产层做三件事：

1. **解析**：把 `.gltf`/`.glb`、图片文件读进内存结构。
2. **句柄化**：用稳定句柄（`MeshHandle`/`TextureHandle`/…）引用资源，与 ECS 的 `Mesh` 组件对接。
3. **上传**：把 CPU 端数据搬到 GPU 缓冲/图像（这一步在 `prism-render` 完成）。

## 稳定句柄：slotmap

引擎用 `slotmap` 生成**稳定、可复制、防悬垂**的句柄，而不是裸索引：

```rust
new_key_type! {
    pub struct MeshHandle;
    pub struct MaterialHandle;
    pub struct TextureHandle;
    pub struct InstanceHandle;
}
```

句柄即使底层槽位被回收也不会变成「指向别处的野指针」——slotmap 的版本机制与 ECS 的 `generation` 异曲同工。

## SceneStore：资产中央仓库

`SceneStore` 按类型存所有资源，并提供 `load_gltf`：

```rust
pub struct SceneStore {
    scenes: SlotMap<SceneHandle, Scene>,
    meshes: SlotMap<MeshHandle, MeshData>,
    materials: SlotMap<MaterialHandle, MaterialData>,
    textures: SlotMap<TextureHandle, TextureData>,
    instances: SlotMap<InstanceHandle, InstanceData>,
}

impl SceneStore {
    pub fn load_gltf(&mut self, path: &Path) -> Result<SceneHandle>;
    pub fn load_gltf_bytes(&mut self, bytes: &[u8], ...) -> Result<SceneHandle>;
    pub fn insert_mesh(&mut self, data: MeshData) -> MeshHandle;
    pub fn mesh(&self, h: MeshHandle) -> Option<&MeshData>;
}
```

:::tip 加载即「构建 ECS 实例」
`load_gltf` 内部把 glTF 的 node → 引擎的 `InstanceData`（含 Transform + MeshHandle + MaterialHandle），再 `add_instance_to_scene`。于是渲染系统 `query` 到的就是真实模型了。
:::

## glTF 加载：用 `gltf` crate

注意 `Cargo.toml` 里 `gltf = "1.4"`——这个版本号是 **crate 的发布流**，不是 glTF 规范版本；该 crate 加载的是 **glTF 2.0**：

```rust
let gltf = gltf::Gltf::from_slice(bytes)?;
for scene in gltf.scenes() {
    for node in scene.nodes() {
        // node.transform() → Transform
        // node.mesh() → 顶点/索引 → MeshData
        // node.mesh().material() → MaterialData
    }
}
```

## 纹理：用 `image` crate 解码

`image = "0.25"` 关掉默认特性只留 png/jpeg，减小体积：

```toml
image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }
```

解码后转成 `TextureData`，缺失时回退为品红（`magenta_fallback`）以便一眼看出贴图没加载：

```rust
pub fn magenta_fallback() -> Self { /* (1,0,1) 品红 */ }
```

:::warn 颜色空间别搞错
glTF 的 baseColor 纹理通常是 **sRGB**，而法线/金属度/粗糙度贴图是 **线性**。上传到 Vulkan 时要用正确的 `VkFormat`（如 `SRGB8`）或在采样时做转换，否则颜色会偏暗/偏亮。引擎的 `TexFormat` 枚举区分了这点。
:::

## 动手练习

:::exercise
1. 读 `crates/prism-asset/src/scene_store.rs` 的 `load_gltf`，画出「glTF node → InstanceData」的映射关系。
2. 用 `image` crate 写一段代码：加载一张 PNG，打印它的宽/高/像素格式。
3. 思考：`MeshData`（CPU 端）和 `MeshHandle`（引用）为什么必须分开？渲染层（`prism-render`）如何消费它们？
4. 把一个真实的 `.glb` 拖进 `assets/`，跑引擎看模型是否出现——这是 M3 场景「被文件替代」的瞬间。
:::

下一章，我们把光照从 Blinn-Phong 升级到物理正确的 PBR + IBL。
