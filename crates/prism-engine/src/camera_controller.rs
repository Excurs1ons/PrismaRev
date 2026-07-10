use crate::camera::OrbitCamera;
use crate::input::{InputState, MouseButton};

/// Reads InputState and applies orbit/zoom to an OrbitCamera.
pub struct OrbitCameraController {
    pub sensitivity: f32,
    pub scroll_sensitivity: f32,
}

impl Default for OrbitCameraController {
    fn default() -> Self {
        Self { sensitivity: 0.005, scroll_sensitivity: 0.1 }
    }
}

impl OrbitCameraController {
    pub fn update(&self, camera: &mut OrbitCamera, input: &InputState) {
        // Left mouse drag → orbit
        if input.mouse_held(MouseButton::Left) {
            let d = input.mouse_delta();
            camera.theta -= d[0] as f32 * self.sensitivity;
            camera.phi   -= d[1] as f32 * self.sensitivity;
            // Clamp elevation to avoid gimbal lock
            camera.phi = camera.phi.clamp(0.01, std::f32::consts::PI - 0.01);
        }
        // Scroll → zoom
        let scroll = input.scroll_delta() as f32;
        if scroll.abs() > 0.0 {
            camera.distance *= 1.0 - scroll * self.scroll_sensitivity;
            camera.distance = camera.distance.max(0.1).min(1000.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::{ElementState, MouseScrollDelta};

    #[test]
    fn default_sensitivity_values() {
        let ctrl = OrbitCameraController::default();
        assert!((ctrl.sensitivity - 0.005).abs() < 1e-9);
        assert!((ctrl.scroll_sensitivity - 0.1).abs() < 1e-9);
    }

    #[test]
    fn left_drag_updates_theta_and_phi() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let mut input = InputState::new();

        // Press left button and move mouse right + down
        input.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        input.handle_mouse_move([100.0, 50.0]); // delta = [100, 50]

        let old_theta = camera.theta;
        let old_phi = camera.phi;
        ctrl.update(&mut camera, &input);

        // theta decreases with rightward drag (positive delta x)
        assert!(camera.theta < old_theta);
        // phi decreases with downward drag (positive delta y)
        assert!(camera.phi < old_phi);
    }

    #[test]
    fn no_drag_does_not_affect_camera() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let input = InputState::new();

        let old_theta = camera.theta;
        let old_phi = camera.phi;
        let old_dist = camera.distance;
        ctrl.update(&mut camera, &input);

        assert_eq!(camera.theta, old_theta);
        assert_eq!(camera.phi, old_phi);
        assert_eq!(camera.distance, old_dist);
    }

    #[test]
    fn scroll_up_zooms_in() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let mut input = InputState::new();

        input.handle_scroll(MouseScrollDelta::LineDelta(0.0, 5.0));
        let old_dist = camera.distance;
        ctrl.update(&mut camera, &input);

        // Positive scroll = zoom in (distance decreases)
        assert!(camera.distance < old_dist);
    }

    #[test]
    fn scroll_down_zooms_out() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let mut input = InputState::new();

        input.handle_scroll(MouseScrollDelta::LineDelta(0.0, -3.0));
        let old_dist = camera.distance;
        ctrl.update(&mut camera, &input);

        assert!(camera.distance > old_dist);
    }

    #[test]
    fn phi_clamped_to_avoid_gimbal_lock() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let mut input = InputState::new();

        // Drag upward to push phi past 0.01
        input.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        input.handle_mouse_move([0.0, -50000.0]); // huge upward drag
        ctrl.update(&mut camera, &input);

        assert!(camera.phi >= 0.01);
        assert!(camera.phi <= std::f32::consts::PI - 0.01);
    }

    #[test]
    fn distance_clamped_to_range() {
        let mut camera = OrbitCamera::new(16.0 / 9.0);
        let ctrl = OrbitCameraController::default();
        let mut input = InputState::new();

        // Extreme zoom in
        input.handle_scroll(MouseScrollDelta::LineDelta(0.0, 9999.0));
        ctrl.update(&mut camera, &input);
        assert!(camera.distance >= 0.1);

        // Extreme zoom out
        input.handle_scroll(MouseScrollDelta::LineDelta(0.0, -9999.0));
        ctrl.update(&mut camera, &input);
        assert!(camera.distance <= 1000.0);
    }
}
