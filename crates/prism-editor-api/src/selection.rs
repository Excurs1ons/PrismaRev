//! 编辑器门面。
//!
//! 引擎的 Selection 操作 编辑器接口；这里「对象」= 当前文档里的
//! **实体下标**。状态是显式的 [`SelectionState`]（由宿主 / 前端持有并持久化），
//! 不放在全局静态里，以免多文档互相污染。
//!
//! | | 这里 |
//! |---|---|
//! | `Selection.objects` / `gameObjects` | [`SelectionState::objects`] |
//! | `Selection.activeObject` / `activeGameObject` | [`SelectionState::active_object`] |
//! | `Selection.Contains` | [`SelectionState::contains`] |
//! | `Selection.GetFiltered(type, mode)` | [`SelectionState::get_filtered`] |
//! | `Selection.activeObject = x` | [`SelectionState::set_active_object`] |

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::scene::PREFAB_LINK_COMPONENT;

/// 过滤模式 = `SelectionMode`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SelectionMode {
    /// 不过滤。
    All,
    /// 仅根实体（`parent == null`）。
    Root,
    /// 仅预制体实例根。
    PrefabRoot,
    /// 仅拥有指定组件的实体。
    HasComponent,
}

/// 过滤条件 = `GetFiltered(Type, SelectionMode)` 的组合。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelectionFilter {
    pub mode: SelectionMode,
    /// `HasComponent` 时使用的组件规范名。
    pub component: Option<String>,
}

impl Default for SelectionFilter {
    fn default() -> Self {
        Self {
            mode: SelectionMode::All,
            component: None,
        }
    }
}

/// 选择状态（实体下标集合 + 活动项）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SelectionState {
    /// 已选实体下标（保持插入顺序）。
    pub indices: Vec<usize>,
    /// 活动实体（Inspector 显示的那个）。
    pub active: Option<usize>,
}

impl SelectionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 清空选择 = `Selection.objects = Array.Empty()`。
    pub fn clear(&mut self) {
        self.indices.clear();
        self.active = None;
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn contains(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }

    /// `Selection.objects`。
    pub fn objects(&self) -> &[usize] {
        &self.indices
    }

    /// `Selection.gameObjects`。
    pub fn game_objects(&self) -> &[usize] {
        &self.indices
    }

    /// `Selection.activeObject`。
    pub fn active_object(&self) -> Option<usize> {
        self.active
    }

    /// `Selection.activeGameObject`。
    pub fn active_game_object(&self) -> Option<usize> {
        self.active
    }

    /// `Selection.activeObject = index`：设为**唯一**选中项（`None` = 清空）。
    pub fn set_active_object(&mut self, index: Option<usize>) {
        match index {
            Some(index) => {
                self.indices = vec![index];
                self.active = Some(index);
            }
            None => self.clear(),
        }
    }

    /// 直接替换整个选择集合（活动项落到最后一个）。
    pub fn set_indices(&mut self, indices: Vec<usize>) {
        self.indices = dedup(indices);
        self.active = self.indices.last().copied();
    }

    /// 追加选中（Ctrl+点选）。
    pub fn add(&mut self, index: usize) {
        if !self.contains(index) {
            self.indices.push(index);
        }
        self.active = Some(index);
    }

    /// 取消选中某一项；被移除的是活动项时活动项回退到最后一个。
    pub fn remove(&mut self, index: usize) {
        self.indices.retain(|value| *value != index);
        if self.active == Some(index) {
            self.active = self.indices.last().copied();
        }
    }

    /// 切换选中状态。
    pub fn toggle(&mut self, index: usize) {
        if self.contains(index) {
            self.remove(index);
        } else {
            self.add(index);
        }
    }

    /// 剔除越界下标（文档被删减后调用）。
    pub fn retain_valid(&mut self, entity_count: usize) {
        self.indices.retain(|index| *index < entity_count);
        if self.active.is_some_and(|index| index >= entity_count) {
            self.active = self.indices.last().copied();
        }
    }

    /// `Selection.GetFiltered`：在选中集合上按 [`SelectionFilter`] 过滤。
    pub fn get_filtered(&self, doc: &Value, filter: &SelectionFilter) -> Vec<usize> {
        let entities = doc.get("entities").and_then(Value::as_array);
        self.indices
            .iter()
            .copied()
            .filter(|index| {
                let Some(entity) = entities.and_then(|list| list.get(*index)) else {
                    return false;
                };
                match filter.mode {
                    SelectionMode::All => true,
                    SelectionMode::Root => entity.get("parent").is_none_or(Value::is_null),
                    SelectionMode::PrefabRoot => entity
                        .get("components")
                        .and_then(|c| c.get(PREFAB_LINK_COMPONENT))
                        .is_some(),
                    SelectionMode::HasComponent => {
                        filter.component.as_deref().is_some_and(|name| {
                            entity.get("components").and_then(|c| c.get(name)).is_some()
                        })
                    }
                }
            })
            .collect()
    }
}

fn dedup(indices: Vec<usize>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(indices.len());
    for index in indices {
        if !out.contains(&index) {
            out.push(index);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Value {
        json!({
            "version": prism_scene::SCENE_DOC_VERSION,
            "entities": [
                {
                    "name": "Root",
                    "parent": null,
                    "transform": {},
                    "components": {
                        PREFAB_LINK_COMPONENT: { "prefab": "assets/prefabs/a.prefab.json" }
                    }
                },
                { "name": "Child", "parent": 0, "transform": {}, "components": { "prism_engine::scene::Camera": {} } },
                { "name": "Other", "parent": null, "transform": {}, "components": {} }
            ]
        })
    }

    #[test]
    fn active_object_is_single_selection() {
        let mut selection = SelectionState::new();
        assert!(selection.is_empty());

        selection.add(1);
        selection.add(2);
        assert_eq!(selection.objects(), &[1, 2]);
        assert_eq!(selection.active_object(), Some(2));

        selection.set_active_object(Some(0));
        assert_eq!(selection.objects(), &[0]);

        selection.toggle(1);
        assert_eq!(selection.objects(), &[0, 1]);
        selection.toggle(1);
        assert_eq!(selection.objects(), &[0]);

        selection.remove(0);
        assert_eq!(selection.active_object(), None);
        assert!(selection.is_empty());
    }

    #[test]
    fn filters_by_mode_and_component() {
        let doc = doc();
        let mut selection = SelectionState::new();
        selection.set_indices(vec![0, 1, 2]);

        assert_eq!(
            selection.get_filtered(&doc, &SelectionFilter::default()),
            vec![0, 1, 2]
        );
        assert_eq!(
            selection.get_filtered(
                &doc,
                &SelectionFilter {
                    mode: SelectionMode::Root,
                    component: None
                }
            ),
            vec![0, 2]
        );
        assert_eq!(
            selection.get_filtered(
                &doc,
                &SelectionFilter {
                    mode: SelectionMode::PrefabRoot,
                    component: None
                }
            ),
            vec![0]
        );
        assert_eq!(
            selection.get_filtered(
                &doc,
                &SelectionFilter {
                    mode: SelectionMode::HasComponent,
                    component: Some("prism_engine::scene::Camera".into())
                }
            ),
            vec![1]
        );
    }

    #[test]
    fn retain_valid_drops_out_of_range() {
        let mut selection = SelectionState::new();
        selection.set_indices(vec![0, 5, 2]);
        assert_eq!(selection.active_object(), Some(2));
        selection.retain_valid(3);
        assert_eq!(selection.objects(), &[0, 2]);
        selection.retain_valid(0);
        assert!(selection.is_empty());
        assert_eq!(selection.active_object(), None);
    }
}
