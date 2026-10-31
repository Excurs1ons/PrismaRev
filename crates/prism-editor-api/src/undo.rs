//! 编辑器门面。
//!
//! 编辑器的撤销/重做在前端按「字段」合并连续输入；这一层提供与之对应的
//! **快照栈**，供宿主（Tauri 后端 / 未来的原生编辑器）以引擎的方式登记操作。
//!
//! | | 这里 |
//! |---|---|
//! | `Undo.RecordObject(obj, name)` | [`UndoStack::record_object`] |
//! | `Undo.RegisterCreatedObjectUndo(obj, name)` | [`UndoStack::register_created_object_undo`] |
//! | `Undo.RegisterFullObjectHierarchyUndo(obj, name)` | [`UndoStack::register_full_object_hierarchy_undo`] |
//! | `Undo.PerformUndo` / `PerformRedo` | [`UndoStack::perform_undo`] / [`UndoStack::perform_redo`] |
//! | `Undo.ClearAll` | [`UndoStack::clear_all`] |
//! | `Undo.CollapseUndoOperations(group)` | [`UndoStack::collapse_undo_operations`] |
//!
//! 快照用泛型 `T`：文档编辑场景用 [`DocUndoStack`]（`T = serde_json::Value`）。

use serde::{Deserialize, Serialize};

/// 默认撤销上限（超出后丢弃最旧的一步）。
pub const DEFAULT_UNDO_LIMIT: usize = 128;

/// 一步操作：同一对象/文档的「之前」与「之后」快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UndoRecord<T> {
    /// 操作名（菜单里显示成 `Undo <label>`）。
    pub label: String,
    pub before: T,
    pub after: T,
}

/// 快照式撤销栈。
#[derive(Debug, Clone, PartialEq)]
pub struct UndoStack<T> {
    records: Vec<UndoRecord<T>>,
    /// 已应用的记录数；`records[index..]` 是重做栈。
    index: usize,
    limit: usize,
}

impl<T: Clone> Default for UndoStack<T> {
    fn default() -> Self {
        Self::new(DEFAULT_UNDO_LIMIT)
    }
}

impl<T: Clone> UndoStack<T> {
    /// 新建栈（`limit == 0` 视为 1）。
    pub fn new(limit: usize) -> Self {
        Self {
            records: Vec::new(),
            index: 0,
            limit: limit.max(1),
        }
    }

    /// 清空 = `Undo.ClearAll`。
    pub fn clear_all(&mut self) {
        self.records.clear();
        self.index = 0;
    }

    /// 登记一步操作 = `Undo.RecordObject`。会丢弃当前的重做栈。
    pub fn record_object(&mut self, label: impl Into<String>, before: T, after: T) {
        self.records.truncate(self.index);
        if self.records.len() >= self.limit {
            self.records.remove(0);
        }
        self.records.push(UndoRecord {
            label: label.into(),
            before,
            after,
        });
        self.index = self.records.len();
    }

    /// `Undo.RegisterCreatedObjectUndo`（语义同 [`Self::record_object`]：
    /// `before` 传创建前的空快照）。
    pub fn register_created_object_undo(&mut self, label: impl Into<String>, before: T, after: T) {
        self.record_object(label, before, after);
    }

    /// `Undo.RegisterFullObjectHierarchyUndo`（整棵层级一起入栈）。
    pub fn register_full_object_hierarchy_undo(
        &mut self,
        label: impl Into<String>,
        before: T,
        after: T,
    ) {
        self.record_object(label, before, after);
    }

    /// 撤销一步，返回应恢复到的快照 = `Undo.PerformUndo`。
    pub fn perform_undo(&mut self) -> Option<&T> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        Some(&self.records[self.index].before)
    }

    /// 重做一步，返回应恢复到的快照 = `Undo.PerformRedo`。
    pub fn perform_redo(&mut self) -> Option<&T> {
        if self.index >= self.records.len() {
            return None;
        }
        let record = &self.records[self.index];
        self.index += 1;
        Some(&record.after)
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index < self.records.len()
    }

    /// 可撤销步数。
    pub fn undo_count(&self) -> usize {
        self.index
    }

    /// 可重做步数。
    pub fn redo_count(&self) -> usize {
        self.records.len() - self.index
    }

    /// 下一步撤销/重做的标签（菜单文字用）。
    pub fn undo_label(&self) -> Option<&str> {
        self.index
            .checked_sub(1)
            .and_then(|index| self.records.get(index))
            .map(|record| record.label.as_str())
    }

    /// 下一步重做的标签。
    pub fn redo_label(&self) -> Option<&str> {
        self.records
            .get(self.index)
            .map(|record| record.label.as_str())
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn records(&self) -> &[UndoRecord<T>] {
        &self.records
    }

    /// 合并最近的 `count` 步（`Undo.CollapseUndoOperations`）。`count < 2` 无操作。
    ///
    /// 先丢弃重做栈，再合并已应用的最后 `count` 步。
    pub fn collapse_undo_operations(&mut self, count: usize) {
        self.records.truncate(self.index);
        let count = count.min(self.index);
        if count < 2 {
            return;
        }
        let start = self.index - count;
        let before = self.records[start].before.clone();
        let after = self.records[self.index - 1].after.clone();
        let label = self.records[self.index - 1].label.clone();
        self.records.splice(
            start..self.index,
            [UndoRecord {
                label,
                before,
                after,
            }],
        );
        self.index = start + 1;
    }
}

/// 文档（`.scene.json` / `.prefab.json`）编辑用的撤销栈。
pub type DocUndoStack = UndoStack<serde_json::Value>;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn undo_redo_walks_the_stack() {
        let mut stack = UndoStack::new(8);
        stack.record_object("Move", json!(0), json!(1));
        stack.record_object("Move", json!(1), json!(2));

        assert_eq!(stack.undo_count(), 2);
        assert_eq!(stack.undo_label(), Some("Move"));

        assert_eq!(stack.perform_undo(), Some(&json!(1)));
        assert_eq!(stack.perform_undo(), Some(&json!(0)));
        assert_eq!(stack.perform_undo(), None);
        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        assert_eq!(stack.perform_redo(), Some(&json!(1)));
        assert_eq!(stack.perform_redo(), Some(&json!(2)));
        assert_eq!(stack.perform_redo(), None);
    }

    #[test]
    fn new_record_truncates_redo_tail() {
        let mut stack = UndoStack::new(8);
        stack.record_object("A", json!(0), json!(1));
        stack.record_object("B", json!(1), json!(2));
        stack.perform_undo();
        assert_eq!(stack.redo_count(), 1);

        stack.record_object("C", json!(1), json!(9));
        assert_eq!(stack.redo_count(), 0);
        assert_eq!(stack.undo_count(), 2);
        assert_eq!(stack.undo_label(), Some("C"));
    }

    #[test]
    fn limit_drops_oldest() {
        let mut stack = UndoStack::new(2);
        stack.record_object("A", json!(0), json!(1));
        stack.record_object("B", json!(1), json!(2));
        stack.record_object("C", json!(2), json!(3));
        assert_eq!(stack.undo_count(), 2);
        assert_eq!(stack.records()[0].label, "B");
        assert_eq!(stack.perform_undo(), Some(&json!(2)));
        assert_eq!(stack.perform_undo(), Some(&json!(1)));
        assert_eq!(stack.perform_undo(), None);
    }

    #[test]
    fn collapse_merges_recent_operations() {
        let mut stack = UndoStack::new(16);
        stack.record_object("Drag", json!(0), json!(1));
        stack.record_object("Drag", json!(1), json!(2));
        stack.record_object("Drag", json!(2), json!(3));
        stack.collapse_undo_operations(3);

        assert_eq!(stack.undo_count(), 1);
        assert_eq!(stack.perform_undo(), Some(&json!(0)));
        assert_eq!(stack.perform_redo(), Some(&json!(3)));

        // count < 2 无操作。
        let mut single = UndoStack::new(4);
        single.record_object("A", json!(0), json!(1));
        single.collapse_undo_operations(1);
        assert_eq!(single.undo_count(), 1);
    }

    #[test]
    fn clear_all_resets() {
        let mut stack = DocUndoStack::default();
        stack.register_full_object_hierarchy_undo("Edit", json!({}), json!({ "a": 1 }));
        stack.clear_all();
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.undo_count(), 0);
    }
}
