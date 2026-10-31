//! Hot-reload entry points (engine 侧再导出)。
//!
//! 真正的轮询器是 [`prism_asset::runtime::HotReloadWatcher`]：它只依赖 `std`，运行时
//! 总是可用，产出 [`HotReloadEvent::PakModified`] 后由宿主调用
//! [`ResourceManager::on_pak_changed`](prism_asset::runtime::ResourceManager::on_pak_changed)
//! 换掉陈旧的文件字节。
//!
//! **引擎自己不轮询**，也不会自动重载任何东西。宿主（编辑器 / 玩家）需要做三步：
//!
//! 1. 轮询 `.pak`（或在重建 `.pak` 的地方直接拿到路径）；
//! 2. `ResourceManager::on_pak_changed(path)` 换掉 reader 并原地刷新常驻资源；
//! 3. `GpuAssetResolver::invalidate_assets(&mut world)` 把实体侧的解析结果打回
//!    「待解析」，下一帧 `prepare_requests` 会重新上传 —— GPU 句柄才是渲染用的那一份。
//!
//! 以前这个文件里有一个只返回 `bool`、没有任何调用点的 `HotReloadWatcher`，注释还写着
//! 「已接线」；那是假的，已删除（#495）。别再往这里加第二个 watcher。

pub use prism_asset::runtime::{HotReloadError, HotReloadEvent, HotReloadWatcher};
