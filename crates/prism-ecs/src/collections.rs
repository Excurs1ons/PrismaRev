//! Unity DOTS（`Unity.Collections`）形状的容器门面。
//!
//! Unity 的 `Native*` 容器是手动管理的非托管内存，用 `Allocator` 指定生命周期，
//! 并需要显式 `Dispose()`。Rust 端没有这段手动内存管理：这里的容器用
//! `Box<[T]>` / `Vec<T>` 做**安全的所有权存储**，`Allocator` 只作为元数据保留
//! （便于读代码时对照 Unity，也便于未来接自定义分配器），释放交给 `Drop`。
//!
//! 因此这是**形状对齐**，不是逐行移植：`Dispose` / `NativeArrayOptions` /
//! `AtomicSafetyHandle` 等 Unity 特有的手动管理 API 不存在。
//!
//! 命名对应（Rust 用 snake_case，括号内为 Unity 原名）：
//! [`NativeArray::len`]（`Length`）、[`NativeArray::copy_from`]（`CopyFrom`）、
//! [`NativeList::add`]（`Add`）、[`NativeReference::value`]（`Value`）、
//! [`NativeSlice`]（`NativeSlice`）。

/// Unity `Allocator`：容器内存的生命周期分类。
///
/// 在 PrismaRev 里它不驱动真实分配，只记录“这个容器在 Unity 中会用什么分配器”，
/// 用于对照和未来的自定义分配器接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Allocator {
    /// 未指定（Unity 的 `Allocator.Invalid`）。
    #[default]
    Invalid,
    /// 不需要释放的分配（`Allocator.None`）。
    None,
    /// 一帧内有效的临时分配（`Allocator.Temp`）。
    Temp,
    /// 一个 job 内有效的临时分配（`Allocator.TempJob`）。
    TempJob,
    /// 长期存活的分配（`Allocator.Persistent`）。
    Persistent,
}

impl Allocator {
    /// 是否是 Unity 认可的分配器（`Allocator.Invalid` 之外都算）。
    pub fn is_valid(self) -> bool {
        self != Allocator::Invalid
    }
}

/// Unity `NativeArray<T>` 形状的定长数组。
///
/// 创建时用 [`Default`] 填充（对应 Unity 默认的 `NativeArrayOptions.ClearMemory`），
/// 下标访问与切片访问都走安全 Rust。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeArray<T> {
    data: Box<[T]>,
    allocator: Allocator,
}

impl<T: Default> NativeArray<T> {
    /// 创建 `len` 个元素的数组（Unity：`new NativeArray<T>(len, allocator)`）。
    pub fn new(len: usize, allocator: Allocator) -> Self {
        let data = std::iter::repeat_with(T::default)
            .take(len)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self { data, allocator }
    }
}

impl<T> NativeArray<T> {
    /// 从已有 `Vec` 接管存储（Unity 没有直接对应物，等价于 `CopyFrom` 后释放源）。
    pub fn from_vec(values: Vec<T>, allocator: Allocator) -> Self {
        Self {
            data: values.into_boxed_slice(),
            allocator,
        }
    }

    /// 元素个数（Unity：`Length`）。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空（Unity：`Length == 0`）。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 创建时记录的分配器。
    pub fn allocator(&self) -> Allocator {
        self.allocator
    }

    /// 只读切片视图。
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    /// 可变切片视图。
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// 按下标读取，越界返回 `None`。
    pub fn get(&self, index: usize) -> Option<&T> {
        self.data.get(index)
    }

    /// 按下标可变读取，越界返回 `None`。
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.data.get_mut(index)
    }

    /// 只读迭代。
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.data.iter()
    }

    /// 可变迭代。
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.data.iter_mut()
    }

    /// 把 `source` 复制到开头，元素个数不变（Unity：`CopyFrom`）。
    ///
    /// # Panics
    ///
    /// `source` 比本数组长时 panic。
    pub fn copy_from(&mut self, source: &[T])
    where
        T: Copy,
    {
        assert!(
            source.len() <= self.data.len(),
            "copy_from source length {} exceeds array length {}",
            source.len(),
            self.data.len()
        );
        self.data[..source.len()].copy_from_slice(source);
    }

    /// 复制成 `Vec`（Unity：`ToArray`）。
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.data.to_vec()
    }
}

impl<T: Clone> NativeArray<T> {
    /// 用同一个值填满整个数组（Unity 没有对应物，常用 `for` 循环代替）。
    pub fn fill(&mut self, value: T) {
        self.data.fill(value);
    }
}

impl<T> std::ops::Index<usize> for NativeArray<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.data[index]
    }
}

impl<T> std::ops::IndexMut<usize> for NativeArray<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.data[index]
    }
}

impl<T> From<Vec<T>> for NativeArray<T> {
    fn from(values: Vec<T>) -> Self {
        Self::from_vec(values, Allocator::Persistent)
    }
}

impl<'a, T> IntoIterator for &'a NativeArray<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut NativeArray<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.iter_mut()
    }
}

/// Unity `NativeSlice<T>` 形状的只读视图。
///
/// Unity 的 `NativeSlice` 也可以写；Rust 端只提供只读视图，可变切片直接用
/// `&mut [T]` / [`NativeArray::as_mut_slice`]。
#[derive(Debug)]
pub struct NativeSlice<'a, T> {
    data: &'a [T],
}

impl<'a, T> NativeSlice<'a, T> {
    /// 从切片创建视图。
    pub fn new(data: &'a [T]) -> Self {
        Self { data }
    }

    /// 元素个数（Unity：`Length`）。
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 底层切片。
    pub fn as_slice(&self) -> &'a [T] {
        self.data
    }

    /// 按下标读取，越界返回 `None`。
    pub fn get(&self, index: usize) -> Option<&'a T> {
        self.data.get(index)
    }

    /// 取子区间（Unity：`Slice(start, length)`），越界返回 `None`。
    pub fn slice(&self, start: usize, length: usize) -> Option<NativeSlice<'a, T>> {
        let end = start.checked_add(length)?;
        self.data.get(start..end).map(NativeSlice::new)
    }

    /// 只读迭代。
    pub fn iter(&self) -> std::slice::Iter<'a, T> {
        self.data.iter()
    }
}

impl<T> Clone for NativeSlice<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for NativeSlice<'_, T> {}

impl<'a, T> From<&'a [T]> for NativeSlice<'a, T> {
    fn from(data: &'a [T]) -> Self {
        Self::new(data)
    }
}

impl<'a, T> IntoIterator for NativeSlice<'a, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.data.iter()
    }
}

/// Unity `NativeList<T>` 形状的可增长列表。
///
/// 底层是 `Vec<T>`，`Capacity` 语义与 Unity 一致（不保证每次 `add` 都重新分配）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeList<T> {
    values: Vec<T>,
    allocator: Allocator,
}

impl<T> NativeList<T> {
    /// 创建空列表（Unity：`new NativeList<T>(allocator)`）。
    pub fn new(allocator: Allocator) -> Self {
        Self {
            values: Vec::new(),
            allocator,
        }
    }

    /// 创建带初始容量的空列表。
    pub fn with_capacity(capacity: usize, allocator: Allocator) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
            allocator,
        }
    }

    /// 从已有 `Vec` 接管存储。
    pub fn from_vec(values: Vec<T>, allocator: Allocator) -> Self {
        Self { values, allocator }
    }

    /// 追加元素（Unity：`Add`）。
    pub fn add(&mut self, value: T) {
        self.values.push(value);
    }

    /// 元素个数（Unity：`Length`）。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 当前容量（Unity：`Capacity`）。
    pub fn capacity(&self) -> usize {
        self.values.capacity()
    }

    /// 创建时记录的分配器。
    pub fn allocator(&self) -> Allocator {
        self.allocator
    }

    /// 只读切片。
    pub fn as_slice(&self) -> &[T] {
        &self.values
    }

    /// 可变切片。
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values
    }

    /// 按下标读取，越界返回 `None`。
    pub fn get(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    /// 按下标可变读取，越界返回 `None`。
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.values.get_mut(index)
    }

    /// 清空元素、保留容量（Unity：`Clear`）。
    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// 删除并返回末尾元素（Unity：`RemoveAt(len - 1)`）。
    pub fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }

    /// 在下标处插入，`index > len` 时返回 `false`（Unity：`Insert`，越界会抛异常）。
    pub fn insert(&mut self, index: usize, value: T) -> bool {
        if index > self.values.len() {
            return false;
        }
        self.values.insert(index, value);
        true
    }

    /// 删除下标处元素并保持顺序，越界返回 `None`（Unity：`RemoveAt`）。
    pub fn remove_at(&mut self, index: usize) -> Option<T> {
        if index >= self.values.len() {
            return None;
        }
        Some(self.values.remove(index))
    }

    /// 只读迭代。
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.values.iter()
    }

    /// 可变迭代。
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.values.iter_mut()
    }
}

impl<T: Ord> NativeList<T> {
    /// 升序排序（Unity：`Sort`）。
    pub fn sort(&mut self) {
        self.values.sort();
    }
}

impl<T> std::ops::Index<usize> for NativeList<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.values[index]
    }
}

impl<T> std::ops::IndexMut<usize> for NativeList<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.values[index]
    }
}

impl<'a, T> IntoIterator for &'a NativeList<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut NativeList<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.iter_mut()
    }
}

/// Unity `NativeReference<T>` 形状的单值容器。
///
/// 常用于 job 之间传递一个值（Unity 里配 `NativeDisableParallelForRestriction`
/// 之类的属性使用）；Rust 端就是一个带分配器元数据的 `Box<T>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeReference<T> {
    value: T,
    allocator: Allocator,
}

impl<T> NativeReference<T> {
    /// 创建引用（Unity：`new NativeReference<T>(value, allocator)`）。
    pub fn new(value: T, allocator: Allocator) -> Self {
        Self { value, allocator }
    }

    /// 读取值（Unity：`Value`）。
    pub fn value(&self) -> &T {
        &self.value
    }

    /// 覆盖值（Unity：`Value = ...`）。
    pub fn set_value(&mut self, value: T) {
        self.value = value;
    }

    /// 取走内部值。
    pub fn into_inner(self) -> T {
        self.value
    }

    /// 创建时记录的分配器。
    pub fn allocator(&self) -> Allocator {
        self.allocator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocator_validity_matches_unity_semantics() {
        assert!(!Allocator::Invalid.is_valid());
        assert!(Allocator::None.is_valid());
        assert!(Allocator::Temp.is_valid());
        assert!(Allocator::TempJob.is_valid());
        assert!(Allocator::Persistent.is_valid());
        assert_eq!(Allocator::default(), Allocator::Invalid);
    }

    #[test]
    fn native_array_clears_memory_and_tracks_allocator() {
        let array = NativeArray::<u32>::new(4, Allocator::TempJob);
        assert_eq!(array.len(), 4);
        assert!(!array.is_empty());
        assert_eq!(array.allocator(), Allocator::TempJob);
        assert_eq!(array.as_slice(), &[0, 0, 0, 0]);
    }

    #[test]
    fn native_array_index_and_copy_from() {
        let mut array = NativeArray::from_vec(vec![1u32, 2, 3], Allocator::Persistent);
        assert_eq!(array[1], 2);
        array[1] = 20;
        array.copy_from(&[7, 8]);
        assert_eq!(array.to_vec(), vec![7, 8, 3]);
        array.fill(5);
        assert_eq!(array.iter().copied().collect::<Vec<_>>(), vec![5, 5, 5]);
        assert_eq!(array.get(9), None);
    }

    #[test]
    fn native_slice_bounds_are_checked() {
        let values = [10, 20, 30, 40];
        let slice = NativeSlice::new(&values);
        assert_eq!(slice.len(), 4);
        assert_eq!(slice.get(2), Some(&30));
        assert_eq!(slice.slice(1, 2).map(|s| s.as_slice()), Some(&[20, 30][..]));
        assert!(slice.slice(3, 2).is_none());
        assert_eq!(slice.iter().copied().sum::<i32>(), 100);
    }

    #[test]
    fn native_list_add_insert_remove_and_sort() {
        let mut list = NativeList::new(Allocator::Temp);
        list.add(3);
        list.add(1);
        assert!(list.insert(1, 2));
        assert!(!list.insert(9, 0));
        assert_eq!(list.as_slice(), &[3, 2, 1]);
        assert_eq!(list.remove_at(0), Some(3));
        assert_eq!(list.remove_at(3), None);
        list.sort();
        assert_eq!(list.as_slice(), &[1, 2]);
        assert_eq!(list.pop(), Some(2));
        assert_eq!(list.allocator(), Allocator::Temp);
    }

    #[test]
    fn native_reference_reads_and_writes() {
        let mut reference = NativeReference::new(5u32, Allocator::Persistent);
        assert_eq!(*reference.value(), 5);
        reference.set_value(9);
        assert_eq!(*reference.value(), 9);
        assert_eq!(reference.allocator(), Allocator::Persistent);
        assert_eq!(reference.into_inner(), 9);
    }
}
