//! Physics 线程 — owns the Rapier simulation 世界
//!
//! The main 线程 sends [`PhysicsStep`] (spawn/despawn/set-transform commands)
//! and receives [`PhysicsResult`] 动力学 body transforms) each 帧
//!
//! # Initial scope
//!
//! * 刚体 bodies only 动力学 / 运动学 / 静态
//! * 球体 / 盒 / capsule / trimesh colliders
//! * No joints, no CCD, no 查询 管线
//!
//! # 状态：骨架（未接线）
//!
//! 线程/消息骨架已就位，但尚未被 `App` 启动与消费；
//! 待 ECS 物理系统接入后再启用。保留以抑制 dead_code 警告。

#![allow(dead_code)]

use std::collections::HashMap;

use rapier3d::dynamics::{
    CCDSolver, IntegrationParameters, IslandManager, RigidBodyBuilder, RigidBodyHandle,
    RigidBodySet, RigidBodyType,
};
use rapier3d::geometry::{BroadPhaseBvh, ColliderBuilder, ColliderSet, NarrowPhase};
use rapier3d::math::{Pose, Rotation, Vector};
use rapier3d::pipeline::PhysicsPipeline;

use flume::{Receiver, Sender};

/// 不透明 实体 identifier shared with the ECS 世界 on the main 线程
pub type EntityId = u64;

// ── Commands (main → physics) ─────────────────────────────────────────

pub struct PhysicsStep {
    pub commands: Vec<PhysicsCommand>,
}

pub enum PhysicsCommand {
    SpawnBody {
        entity: EntityId,
        position: [f32; 3],
        rotation: [f32; 4],
        body_status: PhysicsBodyStatus,
        shape: ColliderDesc,
    },
    DespawnBody {
        entity: EntityId,
    },
    SetTransform {
        entity: EntityId,
        position: [f32; 3],
        rotation: [f32; 4],
    },
    SetVelocity {
        entity: EntityId,
        linear: [f32; 3],
        angular: [f32; 3],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsBodyStatus {
    Dynamic,
    KinematicPosition,
    Static,
}

#[derive(Debug, Clone)]
pub enum ColliderDesc {
    Sphere {
        radius: f32,
    },
    Box {
        half_extents: [f32; 3],
    },
    Capsule {
        half_height: f32,
        radius: f32,
    },
    Trimesh {
        vertices: Vec<[f32; 3]>,
        indices: Vec<u32>,
    },
}

// ── Results (physics → main) ──────────────────────────────────────────

pub struct PhysicsResult {
    pub transforms: Vec<BodyTransform>,
}

pub struct BodyTransform {
    pub entity: EntityId,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub linear_velocity: [f32; 3],
}

// ── helpers ───────────────────────────────────────────────────────────

fn array_to_pose(position: [f32; 3], rotation: [f32; 4]) -> Pose {
    Pose::from_parts(
        Vector::new(position[0], position[1], position[2]),
        Rotation::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]),
    )
}

fn rapier_status(status: PhysicsBodyStatus) -> RigidBodyType {
    match status {
        PhysicsBodyStatus::Dynamic => RigidBodyType::Dynamic,
        PhysicsBodyStatus::KinematicPosition => RigidBodyType::KinematicPositionBased,
        PhysicsBodyStatus::Static => RigidBodyType::Fixed,
    }
}

// ── 线程 entry point ────────────────────────────────────────────────

pub fn physics_thread_main(step_rx: Receiver<PhysicsStep>, result_tx: Sender<PhysicsResult>) {
    log::info!("Physics thread started");

    let mut rigid_body_set = RigidBodySet::new();
    let mut collider_set = ColliderSet::new();
    let mut island_manager = IslandManager::new();
    let mut broad_phase = BroadPhaseBvh::new();
    let mut narrow_phase = NarrowPhase::new();
    let mut impulse_joint_set = rapier3d::dynamics::ImpulseJointSet::new();
    let mut multibody_joint_set = rapier3d::dynamics::MultibodyJointSet::new();
    let mut ccd_solver = CCDSolver;
    let gravity = Vector::new(0.0, -9.81, 0.0);
    let integration_params = IntegrationParameters::default();
    let mut physics_pipeline = PhysicsPipeline::new();

    // 实体 → Rapier handle 映射表
    let mut entity_map: HashMap<EntityId, RigidBodyHandle> = HashMap::new();

    while let Ok(step) = step_rx.recv() {
        // 1. Apply commands.
        for cmd in step.commands {
            match cmd {
                PhysicsCommand::SpawnBody {
                    entity,
                    position,
                    rotation,
                    body_status,
                    shape,
                } => {
                    let body = RigidBodyBuilder::new(rapier_status(body_status))
                        .pose(array_to_pose(position, rotation))
                        .build();
                    let body_handle = rigid_body_set.insert(body);
                    entity_map.insert(entity, body_handle);

                    let collider = match shape {
                        ColliderDesc::Sphere { radius } => ColliderBuilder::ball(radius).build(),
                        ColliderDesc::Box { half_extents } => ColliderBuilder::cuboid(
                            half_extents[0],
                            half_extents[1],
                            half_extents[2],
                        )
                        .build(),
                        ColliderDesc::Capsule {
                            half_height,
                            radius,
                        } => ColliderBuilder::capsule_y(half_height, radius).build(),
                        ColliderDesc::Trimesh { vertices, indices } => {
                            let rapier_vertices: Vec<Vector> = vertices
                                .iter()
                                .map(|v| Vector::new(v[0], v[1], v[2]))
                                .collect();
                            let rapier_indices: Vec<[u32; 3]> = indices
                                .chunks_exact(3)
                                .map(|c| [c[0], c[1], c[2]])
                                .collect();
                            ColliderBuilder::trimesh(rapier_vertices, rapier_indices)
                                .expect("Failed to build trimesh collider")
                                .build()
                        }
                    };
                    collider_set.insert_with_parent(collider, body_handle, &mut rigid_body_set);
                }
                PhysicsCommand::DespawnBody { entity } => {
                    if let Some(handle) = entity_map.remove(&entity) {
                        rigid_body_set.remove(
                            handle,
                            &mut island_manager,
                            &mut collider_set,
                            &mut impulse_joint_set,
                            &mut multibody_joint_set,
                            true,
                        );
                    }
                }
                PhysicsCommand::SetTransform {
                    entity,
                    position,
                    rotation,
                } => {
                    if let Some(handle) = entity_map.get(&entity) {
                        if let Some(body) = rigid_body_set.get_mut(*handle) {
                            body.set_position(array_to_pose(position, rotation), true);
                        }
                    }
                }
                PhysicsCommand::SetVelocity {
                    entity,
                    linear,
                    angular,
                } => {
                    if let Some(handle) = entity_map.get(&entity) {
                        if let Some(body) = rigid_body_set.get_mut(*handle) {
                            body.set_linvel(Vector::new(linear[0], linear[1], linear[2]), true);
                            body.set_angvel(Vector::new(angular[0], angular[1], angular[2]), true);
                        }
                    }
                }
            }
        }

        // 2. Step simulation.
        physics_pipeline.step(
            gravity,
            &integration_params,
            &mut island_manager,
            &mut broad_phase,
            &mut narrow_phase,
            &mut rigid_body_set,
            &mut collider_set,
            &mut impulse_joint_set,
            &mut multibody_joint_set,
            &mut ccd_solver,
            &(), // PhysicsHooks
            &(), // EventHandler
        );

        // 3. Collect 动力学 body transforms.
        let transforms: Vec<BodyTransform> = entity_map
            .iter()
            .filter_map(|(entity, handle)| {
                let body = rigid_body_set.get(*handle)?;
                if !body.is_dynamic() {
                    return None; // only send back dynamic body positions
                }
                let pose = body.position();
                let linvel = body.linvel();
                Some(BodyTransform {
                    entity: *entity,
                    position: [pose.translation.x, pose.translation.y, pose.translation.z],
                    rotation: [
                        pose.rotation.x,
                        pose.rotation.y,
                        pose.rotation.z,
                        pose.rotation.w,
                    ],
                    linear_velocity: [linvel.x, linvel.y, linvel.z],
                })
            })
            .collect();

        let _ = result_tx.send(PhysicsResult { transforms });
    }

    log::info!("Physics thread exiting");
}
