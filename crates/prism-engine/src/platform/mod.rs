//! Abstract platform layer — wraps event-loop lifecycle behind `AppDriver` + `PlatformContext`.
//!
//! Currently backed by winit. When a second backend is added, the public types here
//! become abstract traits or enums.

mod winit_impl;

pub(crate) use winit_impl::*;
