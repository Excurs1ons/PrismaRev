use super::*;

#[test]
fn debug_mode_values() {
    assert_eq!(DebugMode::Final as u32, 0);
    assert_eq!(DebugMode::Normal as u32, 5);
}

#[test]
fn normal_space_cycle() {
    assert_eq!(NormalSpace::World.next(), NormalSpace::View);
    assert_eq!(NormalSpace::View.next(), NormalSpace::Tangent);
    assert_eq!(NormalSpace::Tangent.next(), NormalSpace::World);
}
