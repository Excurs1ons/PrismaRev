use crate::camera::OrbitCamera;
use crate::input::{InputManager, MouseButton};

/// Reads InputManager and applies orbit/zoom to an OrbitCamera.
pub struct OrbitCameraController {
    pub sensitivity: f32,
    pub scroll_sensitivity: f32,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        Self {
            sensitivity: 0.005,
            scroll_sensitivity: 0.1,
        }
    }
}

impl OrbitCameraController {
    pub fn update(&self, camera: &mut OrbitCamera, input: &InputManager) {
        // 左 鼠标 拖拽 → orbit
        if input.mouse_held(MouseButton::Left) {
            let d = input.mouse_delta();
            camera.theta -= d[0] as f32 * self.sensitivity;
            camera.phi -= d[1] as f32 * self.sensitivity;
            // 限定 elevation to avoid gimbal lock
            camera.phi = camera.phi.clamp(0.01, std::f32::consts::PI - 0.01);
        }
        // 滚动 → zoom
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            camera.distance *= 1.0 - scroll * self.scroll_sensitivity;
            camera.distance = camera.distance.clamp(0.1, 1000.0);
        }
    }
}

#[cfg(test)]
#[path = "camera_controller_tests.rs"]
mod tests;

