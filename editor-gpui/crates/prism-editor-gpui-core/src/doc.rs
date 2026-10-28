//! 场景/预制体文档模型（`.scene.json` 的 Rust 镜像）。
//!
//! 与 `editor/src/types.ts` 的 `SceneDoc` / `EntityJson` 一一对应，字段名保持
//! camelCase；`parent` 指向实体的稳定 `id`（v3 起不再用数组下标），因此层级
//! 重排、删除、预制体实例化都不会让父子关系漂移。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

/// 预制体实例链接组件名（编辑器元数据，引擎运行时不查询）。
pub const PREFAB_LINK_COMPONENT: &str = "prism_engine::scene::PrefabInstance";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformJson {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for TransformJson {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl TransformJson {
    /// 平移增量（欧拉角不作为真理源；编辑器只写 position/rotation/scale）。
    pub fn translated(mut self, delta: [f32; 3]) -> Self {
        for (axis, value) in delta.iter().enumerate() {
            self.position[axis] += value;
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    /// 父实体的 `id`（不是数组下标）。
    #[serde(default)]
    pub parent: Option<u32>,
    #[serde(default)]
    pub transform: TransformJson,
    #[serde(default)]
    pub components: Map<String, Value>,
}

impl Default for EntityJson {
    fn default() -> Self {
        Self {
            id: None,
            name: None,
            parent: None,
            transform: TransformJson::default(),
            components: Map::new(),
        }
    }
}

impl EntityJson {
    pub fn display_name(&self) -> String {
        match &self.name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => "GameObject".to_string(),
        }
    }

    pub fn component(&self, name: &str) -> Option<&Value> {
        self.components.get(name)
    }

    pub fn has_component(&self, name: &str) -> bool {
        self.components.contains_key(name)
    }

    pub fn active(&self) -> bool {
        match self.components.get("prism_engine::scene::Active") {
            Some(Value::Object(map)) => map
                .get("value")
                .and_then(|value| value.as_bool())
                .or_else(|| map.get("active").and_then(|value| value.as_bool()))
                .unwrap_or(true),
            _ => true,
        }
    }

    pub fn prefab_link(&self) -> Option<&str> {
        self.components
            .get(PREFAB_LINK_COMPONENT)
            .and_then(|value| value.get("prefab"))
            .and_then(|value| value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDoc {
    pub version: u32,
    #[serde(default)]
    pub entities: Vec<EntityJson>,
}

pub const DOC_VERSION: u32 = 3;

impl Default for SceneDoc {
    fn default() -> Self {
        Self {
            version: DOC_VERSION,
            entities: Vec::new(),
        }
    }
}

impl SceneDoc {
    pub fn from_json_str(text: &str) -> Result<Self, serde_json::Error> {
        let mut doc: SceneDoc = serde_json::from_str(text)?;
        doc.normalize();
        Ok(doc)
    }

    pub fn to_json_string(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// 给缺失 id 的实体按数组顺序补 id，并把 `parent`（旧文档里可能是下标）
    /// 迁移成 id。与 `prism_scene::migrate_scene_doc` 的意图一致。
    pub fn normalize(&mut self) {
        if self.version < DOC_VERSION {
            // v1/v2：parent 是下标。
            let index_to_id: Vec<u32> = (0..self.entities.len()).map(|i| i as u32 + 1).collect();
            for (index, entity) in self.entities.iter_mut().enumerate() {
                if entity.id.is_none() {
                    entity.id = Some(index_to_id[index]);
                }
                entity.parent = entity
                    .parent
                    .and_then(|parent| index_to_id.get(parent as usize).copied());
            }
            self.version = DOC_VERSION;
        }

        let mut next = self
            .entities
            .iter()
            .filter_map(|entity| entity.id)
            .max()
            .unwrap_or(0)
            + 1;
        for entity in &mut self.entities {
            if entity.id.is_none() {
                entity.id = Some(next);
                next += 1;
            }
        }

        // 丢掉指向不存在 id 的 parent（防环在 reparent 时单独检查）。
        let ids: HashSet<u32> = self.entities.iter().filter_map(|e| e.id).collect();
        for entity in &mut self.entities {
            if let Some(parent) = entity.parent {
                if !ids.contains(&parent) {
                    entity.parent = None;
                }
            }
        }
    }

    pub fn next_id(&self) -> u32 {
        self.entities
            .iter()
            .filter_map(|entity| entity.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn index_of_id(&self, id: u32) -> Option<usize> {
        self.entities
            .iter()
            .position(|entity| entity.id == Some(id))
    }

    pub fn id_of_index(&self, index: usize) -> Option<u32> {
        self.entities.get(index).and_then(|entity| entity.id)
    }

    pub fn roots(&self) -> Vec<usize> {
        self.entities
            .iter()
            .enumerate()
            .filter(|(_, entity)| entity.parent.is_none())
            .map(|(index, _)| index)
            .collect()
    }

    /// 直接子实体（数组顺序）。
    pub fn children(&self, index: usize) -> Vec<usize> {
        let Some(id) = self.id_of_index(index) else {
            return Vec::new();
        };
        self.entities
            .iter()
            .enumerate()
            .filter(|(_, entity)| entity.parent == Some(id))
            .map(|(index, _)| index)
            .collect()
    }

    /// 深度优先的可见行（`expanded` 决定是否展开某个实体的子级）。
    pub fn visible_rows(&self, expanded: &HashSet<u32>) -> Vec<(usize, u32)> {
        fn walk(
            doc: &SceneDoc,
            indices: Vec<usize>,
            depth: u32,
            expanded: &HashSet<u32>,
            out: &mut Vec<(usize, u32)>,
        ) {
            for index in indices {
                out.push((index, depth));
                if let Some(id) = doc.id_of_index(index) {
                    if expanded.contains(&id) {
                        walk(doc, doc.children(index), depth + 1, expanded, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(self, self.roots(), 0, expanded, &mut out);
        out
    }

    pub fn is_expandable(&self, index: usize) -> bool {
        !self.children(index).is_empty()
    }

    /// 面包屑路径（根 → 实体）。
    pub fn path_of(&self, index: usize) -> Vec<usize> {
        let mut path = Vec::new();
        let mut current = Some(index);
        while let Some(index) = current {
            path.push(index);
            current = self
                .entities
                .get(index)
                .and_then(|entity| entity.parent.and_then(|parent| self.index_of_id(parent)));
        }
        path.reverse();
        path
    }

    fn descendants(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = self.children(index);
        while let Some(index) = stack.pop() {
            out.push(index);
            stack.extend(self.children(index));
        }
        out
    }

    pub fn add_entity(&mut self, entity: EntityJson) -> usize {
        let mut entity = entity;
        entity.id = Some(self.next_id());
        self.entities.push(entity);
        self.entities.len() - 1
    }

    pub fn add_named_child(&mut self, parent: Option<usize>, name: &str) -> usize {
        let parent_id = parent.and_then(|index| self.id_of_index(index));
        let entity = EntityJson {
            name: Some(name.to_string()),
            parent: parent_id,
            ..Default::default()
        };
        self.add_entity(entity)
    }

    pub fn remove_entities(&mut self, indices: &[usize]) -> Vec<usize> {
        let mut removed: Vec<usize> = indices
            .iter()
            .copied()
            .flat_map(|index| {
                let mut all = vec![index];
                all.extend(self.descendants(index));
                all
            })
            .collect();
        removed.sort_unstable();
        removed.dedup();

        let removed_ids: HashSet<u32> = removed
            .iter()
            .filter_map(|index| self.id_of_index(*index))
            .collect();
        let removed_set: HashSet<usize> = removed.iter().copied().collect();

        // 孤儿子级变根。
        for entity in &mut self.entities {
            if let Some(parent) = entity.parent {
                if removed_ids.contains(&parent) {
                    entity.parent = None;
                }
            }
        }

        let mut kept = Vec::with_capacity(self.entities.len());
        for (index, entity) in self.entities.drain(..).enumerate() {
            if !removed_set.contains(&index) {
                kept.push(entity);
            }
        }
        self.entities = kept;
        removed
    }

    pub fn rename(&mut self, index: usize, name: &str) {
        if let Some(entity) = self.entities.get_mut(index) {
            entity.name = Some(name.to_string());
        }
    }

    /// 把 `index` 挂到 `new_parent` 下（`None` = 根），并插到 `position`。
    /// 拒绝自父与成环。
    pub fn reparent(
        &mut self,
        index: usize,
        new_parent: Option<usize>,
        position: Option<usize>,
    ) -> Result<(), &'static str> {
        if self.entities.get(index).is_none() {
            return Err("实体不存在");
        }
        if let Some(parent) = new_parent {
            if parent == index {
                return Err("不能把实体挂到自己下面");
            }
            if self.descendants(index).contains(&parent) {
                return Err("不能把实体挂到自己的子级下面");
            }
        }

        let parent_id = match new_parent {
            Some(parent) => self.id_of_index(parent).ok_or("父实体不存在")?,
            None => {
                self.entities[index].parent = None;
                if let Some(position) = position {
                    self.move_entity(index, position)?;
                }
                return Ok(());
            }
        };
        self.entities[index].parent = Some(parent_id);
        if let Some(position) = position {
            self.move_entity(index, position)?;
        }
        Ok(())
    }

    /// 移动数组位置（层级树的拖拽排序）；父子关系已由调用方设置。
    pub fn move_entity(&mut self, from: usize, to: usize) -> Result<usize, &'static str> {
        if from >= self.entities.len() {
            return Err("实体不存在");
        }
        let to = to.min(self.entities.len() - 1);
        if from == to {
            return Ok(to);
        }
        let entity = self.entities.remove(from);
        self.entities.insert(to, entity);
        Ok(to)
    }

    /// 复制整棵子树（Unity 的 Ctrl+D）：内部 parent 重映射到新 id，根保持原父级。
    pub fn duplicate_subtree(&mut self, index: usize) -> Option<usize> {
        self.entities.get(index)?;
        // 收集子树（根 + 后代），保持数组顺序，保证父级先于子级。
        let mut selected = vec![index];
        selected.extend(self.descendants(index));
        selected.sort_unstable();

        let base = self.next_id();
        let remap: HashMap<u32, u32> = selected
            .iter()
            .filter_map(|index| self.id_of_index(*index))
            .enumerate()
            .map(|(offset, id)| (id, base + offset as u32))
            .collect();
        let root_id = self.id_of_index(index);

        let start = self.entities.len();
        for old_index in selected {
            let Some(source) = self.entities.get(old_index).cloned() else {
                continue;
            };
            let mut copy = source.clone();
            let old_id = source.id;
            copy.id = old_id.and_then(|id| remap.get(&id).copied());
            copy.parent = if old_id == root_id {
                source.parent
            } else {
                source.parent.and_then(|parent| remap.get(&parent).copied())
            };
            if old_id == root_id {
                copy.name = Some(format!("{} (1)", source.display_name()));
            }
            self.entities.push(copy);
        }
        if self.entities.len() == start {
            return None;
        }
        Some(start)
    }

    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        let source = self.entities.get(index)?.clone();
        let mut copy = source.clone();
        copy.id = Some(self.next_id());
        copy.name = Some(format!("{} (1)", source.display_name()));
        let inserted = self.entities.len();
        self.entities.push(copy);
        Some(inserted)
    }

    pub fn set_active(&mut self, index: usize, active: bool) {
        let Some(entity) = self.entities.get_mut(index) else {
            return;
        };
        let entry = entity
            .components
            .entry("prism_engine::scene::Active".to_string())
            .or_insert_with(|| serde_json::json!({ "value": true }));
        if let Value::Object(map) = entry {
            map.insert("value".to_string(), Value::Bool(active));
        }
    }

    pub fn add_component(&mut self, index: usize, name: &str, value: Value) -> bool {
        let Some(entity) = self.entities.get_mut(index) else {
            return false;
        };
        if entity.components.contains_key(name) {
            return false;
        }
        entity.components.insert(name.to_string(), value);
        true
    }

    pub fn remove_component(&mut self, index: usize, name: &str) -> Option<Value> {
        let entity = self.entities.get_mut(index)?;
        if name == "Transform" {
            return None;
        }
        entity.components.remove(name)
    }

    pub fn component_names(&self, index: usize) -> Vec<String> {
        self.entities
            .get(index)
            .map(|entity| entity.components.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 提取选中实体子树为独立文档（「存为预制体」）。
    /// 根上的预制体实例链接会被剥掉，避免预制体引用自己。
    pub fn extract_subtree(&self, roots: &[usize]) -> SceneDoc {
        self.extract_subtree_inner(roots, true)
    }

    /// 复制 / 剪切用：与 `extract_subtree` 相同，但保留根上的预制体链接，
    /// 这样粘贴出来的仍是同源实例（Unity 的复制预制体实例行为）。
    pub fn extract_subtree_preserving_links(&self, roots: &[usize]) -> SceneDoc {
        self.extract_subtree_inner(roots, false)
    }

    fn extract_subtree_inner(&self, roots: &[usize], strip_root_links: bool) -> SceneDoc {
        let mut selected: Vec<usize> = Vec::new();
        for root in roots {
            select_subtree(self, *root, &mut selected);
        }
        selected.sort_unstable();
        selected.dedup();

        let id_map: HashMap<u32, u32> = selected
            .iter()
            .filter_map(|index| self.id_of_index(*index))
            .enumerate()
            .map(|(new_index, id)| (id, new_index as u32 + 1))
            .collect();

        let root_set: HashSet<u32> = roots
            .iter()
            .filter_map(|index| self.id_of_index(*index))
            .collect();

        let mut doc = SceneDoc::default();
        for index in selected {
            let Some(source) = self.entities.get(index) else {
                continue;
            };
            let mut entity = source.clone();
            let old_id = entity.id;
            entity.id = old_id.and_then(|id| id_map.get(&id).copied());
            entity.parent = entity
                .parent
                .and_then(|parent| id_map.get(&parent).copied())
                .filter(|_parent| {
                    // 根实体不能保留指向外部父级的 parent。
                    old_id.map(|id| !root_set.contains(&id)).unwrap_or(false)
                });
            let is_root = old_id.map(|id| root_set.contains(&id)).unwrap_or(false);
            if strip_root_links && is_root {
                entity.components.remove(PREFAB_LINK_COMPONENT);
            }
            doc.entities.push(entity);
        }
        doc.normalize();
        doc
    }

    /// 预制体实例化：追加到场景末尾、内部父 id 平移、根挂到 `parent`、名字去重。
    pub fn instantiate_prefab(
        &mut self,
        prefab: &SceneDoc,
        parent: Option<usize>,
        prefab_path: &str,
    ) -> Vec<usize> {
        let mut result = Vec::new();
        if prefab.entities.is_empty() {
            return result;
        }

        let base = self.next_id();
        let parent_id = parent.and_then(|index| self.id_of_index(index));
        let root_old_id = prefab.id_of_index(0);
        let name_roots = self.name_set();

        let start = self.entities.len();
        for (position, source) in prefab.entities.iter().enumerate() {
            let mut entity = source.clone();
            let old_id = entity.id.unwrap_or(position as u32 + 1);
            entity.id = Some(base + old_id);
            entity.parent = if Some(old_id) == root_old_id {
                parent_id
            } else {
                entity.parent.map(|parent| base + parent)
            };
            if Some(old_id) == root_old_id {
                let mut name = entity.display_name();
                let mut index = 1;
                while name_roots.contains(&name) {
                    name = format!("{} ({index})", entity.display_name());
                    index += 1;
                }
                entity.name = Some(name);
                if !prefab_path.is_empty() {
                    entity.components.insert(
                        PREFAB_LINK_COMPONENT.to_string(),
                        serde_json::json!({ "prefab": prefab_path }),
                    );
                }
            }
            self.entities.push(entity);
            result.push(start + position);
        }
        self.normalize();
        result
    }

    fn name_set(&self) -> HashSet<String> {
        self.entities
            .iter()
            .map(|entity| entity.display_name())
            .collect()
    }

    /// 把组件里精确等于 `from` 的路径字符串替换成 `to`（资产重命名 / 移动用）。
    /// 返回替换次数。
    pub fn rewrite_reference(&mut self, from: &str, to: &str) -> usize {
        let mut count = 0;
        for entity in &mut self.entities {
            for value in entity.components.values_mut() {
                rewrite_value(value, from, to, &mut count);
            }
        }
        count
    }

    pub fn prefab_links(&self) -> Vec<(usize, String)> {
        self.entities
            .iter()
            .enumerate()
            .filter_map(|(index, entity)| {
                entity
                    .prefab_link()
                    .map(|prefab| (index, prefab.to_string()))
            })
            .collect()
    }

    /// 供资产管理器/引用检索用：扫描所有字符串字段里引用了 `path` 的位置。
    pub fn field_references(&self, path: &str) -> Vec<(usize, String, String)> {
        let mut out = Vec::new();
        for (index, entity) in self.entities.iter().enumerate() {
            for (component, value) in &entity.components {
                collect_references(value, path, &mut |field, value| {
                    out.push((index, format!("{component}.{field}"), value.to_string()));
                });
            }
        }
        out
    }
}

fn rewrite_value(value: &mut Value, from: &str, to: &str, count: &mut usize) {
    match value {
        Value::String(text) if text == from => {
            *text = to.to_string();
            *count += 1;
        }
        Value::Array(items) => {
            for item in items {
                rewrite_value(item, from, to, count);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                rewrite_value(item, from, to, count);
            }
        }
        _ => {}
    }
}

fn collect_references(value: &Value, needle: &str, out: &mut impl FnMut(String, &str)) {
    fn walk(value: &Value, needle: &str, path: &str, out: &mut impl FnMut(String, &str)) {
        match value {
            Value::String(text) => {
                if text.contains(needle) {
                    out(path.to_string(), text);
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, needle, &format!("{path}[{index}]"), out);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    let next = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(item, needle, &next, out);
                }
            }
            _ => {}
        }
    }
    walk(value, needle, "", out);
}

fn select_subtree(doc: &SceneDoc, index: usize, out: &mut Vec<usize>) {
    if out.contains(&index) {
        return;
    }
    out.push(index);
    for child in doc.children(index) {
        select_subtree(doc, child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_tree() -> SceneDoc {
        let mut doc = SceneDoc::default();
        let root = doc.add_named_child(None, "Root");
        let child = doc.add_named_child(Some(root), "Child");
        let _grand = doc.add_named_child(Some(child), "Grand");
        let _sibling = doc.add_named_child(None, "Sibling");
        doc
    }

    #[test]
    fn normalize_assigns_stable_ids_and_migrates_index_parents() {
        let mut doc: SceneDoc = serde_json::from_str(
            r#"{"version":1,"entities":[
                {"name":"A","transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}},
                {"name":"B","parent":0,"transform":{"position":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}}
            ]}"#,
        )
        .unwrap();
        doc.normalize();
        assert_eq!(doc.version, DOC_VERSION);
        assert_eq!(doc.entities[0].id, Some(1));
        assert_eq!(doc.entities[1].id, Some(2));
        assert_eq!(doc.entities[1].parent, Some(1), "下标 0 迁移为 id 1");
    }

    #[test]
    fn visible_rows_follow_expansion() {
        let doc = doc_with_tree();
        let mut expanded = HashSet::new();
        let rows = doc.visible_rows(&expanded);
        assert_eq!(rows.len(), 2, "默认只显示两个根");

        let root_id = doc.entities[0].id.unwrap();
        expanded.insert(root_id);
        let rows = doc.visible_rows(&expanded);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].1, 1, "子级缩进一层");
    }

    #[test]
    fn reparent_rejects_self_and_cycles() {
        let mut doc = doc_with_tree();
        assert!(doc.reparent(0, Some(0), None).is_err());
        assert!(doc.reparent(0, Some(1), None).is_err(), "不能挂到子级");
        assert!(doc.reparent(0, Some(3), None).is_ok());
        assert_eq!(doc.entities[0].parent, doc.entities[3].id);
    }

    #[test]
    fn remove_entities_promotes_orphans_and_keeps_ids() {
        let mut doc = doc_with_tree();
        let child_id = doc.entities[1].id.unwrap();
        doc.remove_entities(&[1]);
        // Grand 是 Child 的子级，应一起删除。
        assert_eq!(doc.entities.len(), 2);
        assert!(!doc.entities.iter().any(|e| e.id == Some(child_id)));

        let mut doc = doc_with_tree();
        doc.remove_entities(&[0]);
        // Root 的子级 Child/Grand 一起删除，只剩 Sibling。
        assert_eq!(doc.entities.len(), 1);
        assert_eq!(doc.entities[0].display_name(), "Sibling");
    }

    #[test]
    fn delete_parent_with_child_only_keeps_unrelated() {
        let mut doc = SceneDoc::default();
        let a = doc.add_named_child(None, "A");
        let b = doc.add_named_child(Some(a), "B");
        doc.remove_entities(&[a]);
        assert!(doc.entities.is_empty());
        assert_ne!(b, usize::MAX);
    }

    #[test]
    fn duplicate_subtree_remaps_children_and_keeps_root_parent() {
        let mut doc = doc_with_tree();
        let duplicated = doc.duplicate_subtree(0).expect("duplicate");
        assert_eq!(doc.entities.len(), 7, "3 个原实体 + 3 个复制 + sibling");
        assert_eq!(doc.entities[duplicated].display_name(), "Root (1)");
        assert!(doc.entities[duplicated].parent.is_none());
        let root_id = doc.entities[duplicated].id.unwrap();
        let children: Vec<&EntityJson> = doc
            .entities
            .iter()
            .filter(|entity| entity.parent == Some(root_id))
            .collect();
        assert_eq!(children.len(), 1, "直接子级指向新根");
        assert_eq!(children[0].display_name(), "Child");
        let child_id = children[0].id.unwrap();
        assert!(doc
            .entities
            .iter()
            .any(|entity| entity.parent == Some(child_id)));
        // id 全局唯一。
        let ids: HashSet<u32> = doc.entities.iter().filter_map(|e| e.id).collect();
        assert_eq!(ids.len(), doc.entities.len());
    }

    #[test]
    fn move_entity_reorders_array() {
        let mut doc = doc_with_tree();
        let moved = doc.move_entity(3, 0).unwrap();
        assert_eq!(moved, 0);
        assert_eq!(doc.entities[0].display_name(), "Sibling");
        assert_eq!(doc.entities.len(), 4);
    }

    #[test]
    fn extract_subtree_strips_prefab_link_and_remapps_ids() {
        let mut doc = doc_with_tree();
        doc.entities[0].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({"prefab": "assets/prefabs/a.prefab.json"}),
        );
        let prefab = doc.extract_subtree(&[0]);
        assert_eq!(prefab.entities.len(), 3);
        assert_eq!(prefab.entities[0].id, Some(1));
        assert!(prefab.entities[0].parent.is_none());
        assert!(prefab.entities[0]
            .components
            .get(PREFAB_LINK_COMPONENT)
            .is_none());
        assert_eq!(prefab.entities[1].parent, Some(1));
    }

    #[test]
    fn extract_subtree_keeps_nested_instance_links() {
        let mut doc = doc_with_tree();
        doc.entities[1].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({"prefab": "assets/prefabs/child.prefab.json"}),
        );
        let prefab = doc.extract_subtree(&[0]);
        assert_eq!(
            prefab.entities[1].prefab_link(),
            Some("assets/prefabs/child.prefab.json")
        );
        assert!(prefab.entities[0].prefab_link().is_none());
    }

    #[test]
    fn extract_subtree_preserving_links_keeps_root_link() {
        let mut doc = doc_with_tree();
        doc.entities[0].components.insert(
            PREFAB_LINK_COMPONENT.to_string(),
            serde_json::json!({"prefab": "assets/prefabs/root.prefab.json"}),
        );
        let copy = doc.extract_subtree_preserving_links(&[0]);
        assert_eq!(
            copy.entities[0].prefab_link(),
            Some("assets/prefabs/root.prefab.json")
        );
        let stripped = doc.extract_subtree(&[0]);
        assert!(stripped.entities[0].prefab_link().is_none());
    }

    #[test]
    fn instantiate_prefab_remaps_ids_and_dedupes_names() {
        let mut scene = doc_with_tree();
        let prefab = scene.extract_subtree(&[0]);
        let inserted = scene.instantiate_prefab(&prefab, None, "assets/prefabs/root.prefab.json");
        assert_eq!(inserted.len(), 3);
        let root = &scene.entities[inserted[0]];
        assert_eq!(root.display_name(), "Root (1)");
        assert_eq!(root.prefab_link(), Some("assets/prefabs/root.prefab.json"));
        // 新的 id 不与旧的重叠。
        let ids: HashSet<u32> = scene.entities.iter().filter_map(|e| e.id).collect();
        assert_eq!(ids.len(), scene.entities.len());
    }

    #[test]
    fn field_references_walk_nested_values() {
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Mesh");
        doc.add_component(
            index,
            "prism_engine::scene::MeshRenderer",
            serde_json::json!({
                "mesh_path": "assets/meshes/a.rmes",
                "materials": ["assets/materials/m.rtex"]
            }),
        );
        let refs = doc.field_references("assets/meshes/a.rmes");
        assert_eq!(refs.len(), 1);
        assert!(refs[0].1.ends_with("mesh_path"));
        assert_eq!(doc.field_references("assets/materials/m.rtex").len(), 1);
        assert_eq!(doc.field_references("nope").len(), 0);
    }

    #[test]
    fn rewrite_reference_replaces_exact_paths() {
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Mesh");
        doc.add_component(
            index,
            "prism_engine::scene::MeshRenderer",
            serde_json::json!({
                "mesh_path": "assets/meshes/a.rmes",
                "materials": ["assets/materials/m.rtex", "assets/materials/a.rmes.extra"]
            }),
        );
        let count = doc.rewrite_reference("assets/meshes/a.rmes", "assets/meshes/b.rmes");
        assert_eq!(count, 1, "只替换精确匹配，不做子串替换");
        assert_eq!(
            doc.entities[0].components["prism_engine::scene::MeshRenderer"]["mesh_path"],
            serde_json::json!("assets/meshes/b.rmes")
        );
        let count = doc.rewrite_reference("assets/materials/m.rtex", "assets/materials/n.rtex");
        assert_eq!(count, 1);
    }

    #[test]
    fn active_flag_toggles_component() {
        let mut doc = doc_with_tree();
        assert!(doc.entities[0].active());
        doc.set_active(0, false);
        assert!(!doc.entities[0].active());
        doc.set_active(0, true);
        assert!(doc.entities[0].active());
    }
}
