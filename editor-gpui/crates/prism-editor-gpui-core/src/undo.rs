//! 撤销 / 重做栈。
//!
//! 两类命令：
//! - `SetField`：检查器里的一次字段写入。同一个实体 + 组件 + 路径在
//!   `coalesce_ms` 内连续改动会合并成一步（拖动滑杆只产生一次撤销）。
//! - `Snapshot`：结构性编辑（增删实体 / 重排父子 / 预制体实例化 / 粘贴组件）。
//!   文档很小，整文档快照比逐操作求逆更不容易出错。

use crate::doc::{SceneDoc, TransformJson};
use crate::schema::set_at_path;
use serde_json::Value;

const DEFAULT_LIMIT: usize = 200;
const DEFAULT_COALESCE_MS: u64 = 800;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    SetField {
        entity_id: u32,
        component: String,
        path: String,
        before: Value,
        after: Value,
    },
    /// Transform 是作者 JSON 的结构字段（不在 `components` 里），单独一类命令。
    Transform {
        entity_id: u32,
        before: TransformJson,
        after: TransformJson,
    },
    Snapshot {
        before: Box<SceneDoc>,
        after: Box<SceneDoc>,
    },
}

impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Command::SetField { .. } => "修改字段",
            Command::Transform { .. } => "修改 Transform",
            Command::Snapshot { .. } => "编辑场景",
        }
    }
}

fn apply_field(
    doc: &mut SceneDoc,
    entity_id: u32,
    component: &str,
    path: &str,
    value: &Value,
) -> bool {
    let Some(index) = doc.index_of_id(entity_id) else {
        return false;
    };
    let Some(root) = doc.entities[index].components.get_mut(component) else {
        return false;
    };
    set_at_path(root, path, value.clone())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: Command,
    pub label: String,
    coalesce: Option<String>,
    at_ms: u64,
}

impl Entry {
    pub fn is_field_edit(&self) -> bool {
        matches!(
            self.command,
            Command::SetField { .. } | Command::Transform { .. }
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    coalesce_ms: u64,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: DEFAULT_LIMIT,
            coalesce_ms: DEFAULT_COALESCE_MS,
        }
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(1);
    }

    pub fn set_coalesce_ms(&mut self, ms: u64) {
        self.coalesce_ms = ms;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|entry| entry.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }

    /// 最近到最早的操作标签（「历史」面板）。
    pub fn history(&self) -> Vec<&str> {
        self.undo
            .iter()
            .rev()
            .map(|entry| entry.label.as_str())
            .collect()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// 一次字段写入。`entity_id + component + path` 相同且在合并窗口内时，
    /// 直接把上一步的 `after` 更新成新值（保留最初的 `before`）。
    #[allow(clippy::too_many_arguments)]
    pub fn push_field(
        &mut self,
        doc: &mut SceneDoc,
        entity_id: u32,
        component: &str,
        path: &str,
        before: Value,
        after: Value,
        now_ms: u64,
    ) {
        if before == after {
            return;
        }
        let coalesce = format!("{entity_id}/{component}/{path}");
        if let Some(last) = self.undo.last_mut() {
            if last.coalesce.as_deref() == Some(coalesce.as_str())
                && now_ms.saturating_sub(last.at_ms) <= self.coalesce_ms
            {
                if let Command::SetField {
                    after: last_after, ..
                } = &mut last.command
                {
                    *last_after = after.clone();
                    last.at_ms = now_ms;
                    self.redo.clear();
                    apply_field(doc, entity_id, component, path, &after);
                    return;
                }
            }
        }

        let command = Command::SetField {
            entity_id,
            component: component.to_string(),
            path: path.to_string(),
            before,
            after: after.clone(),
        };
        apply_field(doc, entity_id, component, path, &after);
        self.undo.push(Entry {
            label: format!("{} · {}", short_component(component), path),
            command,
            coalesce: Some(coalesce),
            at_ms: now_ms,
        });
        self.redo.clear();
        self.trim();
    }

    /// Transform 的一次拖动：同一实体的连续改动合并成一步。
    pub fn push_transform(
        &mut self,
        doc: &mut SceneDoc,
        entity_id: u32,
        before: TransformJson,
        after: TransformJson,
        now_ms: u64,
    ) {
        if before == after {
            return;
        }
        let coalesce = format!("transform/{entity_id}");
        if let Some(last) = self.undo.last_mut() {
            if last.coalesce.as_deref() == Some(coalesce.as_str())
                && now_ms.saturating_sub(last.at_ms) <= self.coalesce_ms
            {
                if let Command::Transform {
                    after: last_after, ..
                } = &mut last.command
                {
                    *last_after = after;
                    last.at_ms = now_ms;
                    self.redo.clear();
                    apply_transform(doc, entity_id, &after);
                    return;
                }
            }
        }
        apply_transform(doc, entity_id, &after);
        self.undo.push(Entry {
            label: "修改 Transform".to_string(),
            command: Command::Transform {
                entity_id,
                before,
                after,
            },
            coalesce: Some(coalesce),
            at_ms: now_ms,
        });
        self.redo.clear();
        self.trim();
    }

    /// 结构性编辑：把 `before` 快照与编辑后的文档整体记成一步。
    pub fn push_snapshot(&mut self, label: impl Into<String>, before: SceneDoc, after: &SceneDoc) {
        if before == *after {
            return;
        }
        self.undo.push(Entry {
            label: label.into(),
            command: Command::Snapshot {
                before: Box::new(before),
                after: Box::new(after.clone()),
            },
            coalesce: None,
            at_ms: 0,
        });
        self.redo.clear();
        self.trim();
    }

    pub fn undo(&mut self, doc: &mut SceneDoc) -> Option<String> {
        let mut entry = self.undo.pop()?;
        match &entry.command {
            Command::SetField {
                entity_id,
                component,
                path,
                before,
                ..
            } => {
                apply_field(doc, *entity_id, component, path, before);
            }
            Command::Transform {
                entity_id, before, ..
            } => {
                apply_transform(doc, *entity_id, before);
            }
            Command::Snapshot { before, .. } => {
                *doc = (**before).clone();
            }
        }
        entry.coalesce = None;
        let label = entry.label.clone();
        self.redo.push(entry);
        Some(label)
    }

    pub fn redo(&mut self, doc: &mut SceneDoc) -> Option<String> {
        let mut entry = self.redo.pop()?;
        match &entry.command {
            Command::SetField {
                entity_id,
                component,
                path,
                after,
                ..
            } => {
                apply_field(doc, *entity_id, component, path, after);
            }
            Command::Transform {
                entity_id, after, ..
            } => {
                apply_transform(doc, *entity_id, after);
            }
            Command::Snapshot { after, .. } => {
                *doc = (**after).clone();
            }
        }
        entry.coalesce = None;
        let label = entry.label.clone();
        self.undo.push(entry);
        Some(label)
    }

    fn trim(&mut self) {
        let limit = self.limit;
        if self.undo.len() > limit {
            let excess = self.undo.len() - limit;
            self.undo.drain(0..excess);
        }
    }
}

fn apply_transform(doc: &mut SceneDoc, entity_id: u32, transform: &TransformJson) {
    if let Some(index) = doc.index_of_id(entity_id) {
        doc.entities[index].transform = *transform;
    }
}

fn short_component(component: &str) -> &str {
    component.rsplit("::").next().unwrap_or(component)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{SceneDoc, TransformJson};
    use serde_json::json;

    fn doc_with_camera() -> (SceneDoc, u32) {
        let mut doc = SceneDoc::default();
        let index = doc.add_named_child(None, "Camera");
        doc.add_component(
            index,
            "prism_engine::scene::Camera",
            json!({ "fov_y_degrees": 60.0, "near": 0.1 }),
        );
        let id = doc.entities[index].id.unwrap();
        (doc, id)
    }

    #[test]
    fn field_edits_coalesce_within_window() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(61.0),
            0,
        );
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(61.0),
            json!(70.0),
            100,
        );
        assert_eq!(stack.history().len(), 1, "同一字段连续输入合并成一步");
        assert_eq!(stack.history()[0], "Camera · fov_y_degrees");

        // 撤销回到最初的 60。
        stack.undo(&mut doc);
        let camera = doc.entities[0]
            .components
            .get("prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(camera["fov_y_degrees"], json!(60.0));
        assert!(stack.can_redo());

        stack.redo(&mut doc);
        let camera = doc.entities[0]
            .components
            .get("prism_engine::scene::Camera")
            .unwrap();
        assert_eq!(camera["fov_y_degrees"], json!(70.0));
    }

    #[test]
    fn different_fields_or_stale_edits_create_new_steps() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(60.0),
            json!(70.0),
            10,
        );
        assert_eq!(stack.history().len(), 2);

        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "fov_y_degrees",
            json!(70.0),
            json!(80.0),
            10_000,
        );
        assert_eq!(stack.history().len(), 3, "超出合并窗口算新一步");
    }

    #[test]
    fn new_edit_clears_redo() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.2),
            0,
        );
        stack.undo(&mut doc);
        assert!(stack.can_redo());
        stack.push_field(
            &mut doc,
            id,
            "prism_engine::scene::Camera",
            "near",
            json!(0.1),
            json!(0.3),
            5_000,
        );
        assert!(!stack.can_redo(), "新编辑应清空 redo");
    }

    #[test]
    fn snapshot_undo_redo_restores_whole_doc() {
        let (mut doc, _) = doc_with_camera();
        let before = doc.clone();
        doc.add_named_child(None, "Cube");
        let mut stack = UndoStack::new();
        stack.push_snapshot("新建实体", before.clone(), &doc);
        assert_eq!(doc.entities.len(), 2);

        stack.undo(&mut doc);
        assert_eq!(doc.entities.len(), 1);
        stack.redo(&mut doc);
        assert_eq!(doc.entities.len(), 2);
    }

    #[test]
    fn equal_snapshots_are_ignored() {
        let (doc, _) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_snapshot("无变化", doc.clone(), &doc);
        assert!(!stack.can_undo());
    }

    #[test]
    fn limit_drops_oldest_entries() {
        let (mut doc, id) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.set_limit(3);
        for step in 0..5 {
            stack.push_field(
                &mut doc,
                id,
                "prism_engine::scene::Camera",
                "near",
                json!(step as f64),
                json!(step as f64 + 1.0),
                step as u64 * 10_000,
            );
        }
        assert_eq!(stack.history().len(), 3);
    }

    #[test]
    fn transform_edits_coalesce_and_undo() {
        let (mut doc, id) = doc_with_camera();
        let before = doc.entities[0].transform;
        let mut after = before;
        after.position[0] = 5.0;
        let mut stack = UndoStack::new();
        stack.push_transform(&mut doc, id, before, after, 0);
        assert_eq!(doc.entities[0].transform.position[0], 5.0);

        let mut second = after;
        second.position[0] = 7.0;
        stack.push_transform(&mut doc, id, after, second, 100);
        assert_eq!(stack.history().len(), 1, "同一实体的拖动合并");
        assert_eq!(doc.entities[0].transform.position[0], 7.0);

        stack.undo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 0.0);
        stack.redo(&mut doc);
        assert_eq!(doc.entities[0].transform.position[0], 7.0);
    }

    #[test]
    fn missing_entity_is_silently_skipped() {
        let (mut doc, _) = doc_with_camera();
        let mut stack = UndoStack::new();
        stack.push_field(&mut doc, 999, "c", "p", json!(1), json!(2), 0);
        stack.undo(&mut doc);
        assert_eq!(doc.entities.len(), 1);
    }
}
