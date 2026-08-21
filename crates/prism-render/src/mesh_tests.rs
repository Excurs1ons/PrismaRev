use super::*;

#[test]
fn vertex_stride_is_60() {
    // position(3) + normal(3) + color(3) + uv(2) + tangent(4) = 15 floats = 60 字节
    assert_eq!(std::mem::size_of::<Vertex>(), 60);
    assert_eq!(Vertex::binding_description().stride, 60);
}

#[test]
fn vertex_attribute_offsets() {
    let attrs = Vertex::attribute_descriptions();
    let f = std::mem::size_of::<f32>() as u32;
    assert_eq!(attrs[0].location, 0);
    assert_eq!(attrs[0].offset, 0);
    assert_eq!(attrs[1].location, 1);
    assert_eq!(attrs[1].offset, 3 * f);
    assert_eq!(attrs[2].location, 2);
    assert_eq!(attrs[2].offset, 6 * f);
    assert_eq!(attrs[3].location, 3);
    assert_eq!(attrs[3].offset, 9 * f);
    assert_eq!(attrs[4].location, 4);
    assert_eq!(attrs[4].offset, 11 * f);
}
