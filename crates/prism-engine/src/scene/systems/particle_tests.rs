//! `particle_system` / `scene_particles_into` 测试。

use super::*;
use crate::scene::components::LocalTransform;

fn spawn_emitter(world: &mut World, system: ParticleSystem, position: [f32; 3]) -> Entity {
    let entity = world.spawn();
    world.insert(
        entity,
        LocalTransform {
            position: glam::Vec3::from_array(position),
            ..LocalTransform::default()
        },
    );
    world.insert(
        entity,
        LocalToWorld(glam::Mat4::from_translation(glam::Vec3::from_array(
            position,
        ))),
    );
    world.insert(entity, system);
    entity
}

/// 按 `MAX_STEP` 的步长推进 `seconds`，模拟真实帧循环（`particle_system`
/// 自己会把更大的 `dt` 截断）。
fn run(world: &mut World, seconds: f32) {
    let steps = (seconds / MAX_STEP).round().max(1.0) as usize;
    for _ in 0..steps {
        particle_system(world, MAX_STEP);
    }
}

fn particles_of(world: &World, entity: Entity) -> Vec<Particle> {
    world
        .get::<ParticleEmitter>(entity)
        .map(|emitter| emitter.particles.clone())
        .unwrap_or_default()
}

fn fire() -> ParticleSystem {
    ParticleSystem {
        emission_rate: 10.0,
        lifetime: [1.0, 1.0],
        start_speed: [0.0, 0.0],
        start_size: [0.5, 0.5],
        gravity: [0.0, 0.0, 0.0],
        drag: 0.0,
        shape: ParticleShape::Point,
        ..ParticleSystem::default()
    }
}

#[test]
fn emission_follows_the_rate_and_is_deterministic() {
    let mut world = World::new();
    let entity = spawn_emitter(&mut world, fire(), [0.0, 0.0, 0.0]);

    // 10 / 秒 × 0.5 秒 = 5 个。
    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 5);

    // 同样的种子在别的世界里给出同样的位置。
    let mut other = World::new();
    let other_entity = spawn_emitter(&mut other, fire(), [0.0, 0.0, 0.0]);
    run(&mut other, 0.5);
    let a = particles_of(&world, entity);
    let b = particles_of(&other, other_entity);
    assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(b.iter()) {
        assert_eq!(left.position, right.position);
    }
}

#[test]
fn particles_die_after_their_lifetime() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 4.0,
            ..fire()
        },
        [0.0; 3],
    );

    // 发射 0.5 秒 = 2 个，然后在没有新发射的情况下跑完寿命。
    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 2);

    let mut system = fire();
    system.emission_rate = 0.0;
    world.insert(entity, system);
    run(&mut world, 1.5);
    assert!(particles_of(&world, entity).is_empty());
}

#[test]
fn a_short_emission_stops_after_its_duration() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            duration: 0.2,
            looping: false,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.1);
    let after_first = particles_of(&world, entity).len();
    run(&mut world, 0.1);
    let after_second = particles_of(&world, entity).len();
    run(&mut world, 0.5);
    let after_third = particles_of(&world, entity).len();

    assert_eq!(after_first, 1);
    assert!(after_second >= 2, "duration 内应至少再发一个粒子");
    assert_eq!(
        after_second, after_third,
        "duration 结束后不应再发射，短寿命粒子也还没死"
    );
}

#[test]
fn gravity_and_drag_change_the_velocity() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 1,
            gravity: [0.0, -10.0, 0.0],
            drag: 0.0,
            start_speed: [0.0, 0.0],
            ..fire()
        },
        [0.0; 3],
    );

    // 第一个 0.1 秒只出生（出生在积分之后），第二个 0.1 秒才开始下落。
    run(&mut world, 0.2);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    // 半隐式积分：先加 v，再走位置 → v = g * t = -1，位移 ≈ -0.1。
    assert!((particles[0].velocity.y + 1.0).abs() < 1e-4);
    assert!(particles[0].position.y < 0.0);
}

#[test]
fn the_cap_limits_the_number_of_live_particles() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 1000.0,
            max_particles: 5,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.5);
    assert_eq!(particles_of(&world, entity).len(), 5);
}

#[test]
fn a_paused_emitter_keeps_its_particles_in_place() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            start_speed: [3.0, 3.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.2);
    let before = particles_of(&world, entity);
    assert!(!before.is_empty());

    let mut paused = fire();
    paused.playing = false;
    paused.start_speed = [3.0, 3.0];
    world.insert(entity, paused);
    run(&mut world, 0.5);
    assert_eq!(before, particles_of(&world, entity));
}

#[test]
fn the_emitter_transform_offsets_the_spawn_position() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 1,
            start_speed: [0.0, 0.0],
            shape_radius: 0.0,
            shape: ParticleShape::Sphere,
            ..fire()
        },
        [10.0, 2.0, -3.0],
    );

    particle_system(&mut world, 0.1);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    assert_eq!(particles[0].position, glam::Vec3::new(10.0, 2.0, -3.0));
}

#[test]
fn a_cone_keeps_its_particles_inside_the_half_angle() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 200.0,
            lifetime: [10.0, 10.0],
            cone_angle_degrees: 15.0,
            shape: ParticleShape::Cone,
            shape_radius: 0.0,
            start_speed: [1.0, 1.0],
            ..fire()
        },
        [0.0; 3],
    );

    run(&mut world, 0.5);
    let particles = particles_of(&world, entity);
    assert!(!particles.is_empty());
    let limit = 15.0_f32.to_radians().cos();
    for particle in particles {
        let dir = particle.velocity.normalize();
        assert!(
            dir.y >= limit - 1e-4,
            "cone 只允许本地 +Y 半角内的方向，得到 {dir:?}"
        );
    }
}

#[test]
fn colors_fade_from_start_to_end() {
    let system = ParticleSystem {
        start_color: [1.0, 0.5, 0.0, 1.0],
        end_color: [0.0, 0.0, 1.0, 0.0],
        ..ParticleSystem::default()
    };
    let particle = Particle {
        position: glam::Vec3::ZERO,
        velocity: glam::Vec3::ZERO,
        age: 0.5,
        lifetime: 1.0,
        size: 1.0,
    };
    let color = particle_color(&system, &particle);
    assert!((color[0] - 0.5).abs() < 1e-6);
    assert!((color[1] - 0.25).abs() < 1e-6);
    assert!((color[2] - 0.5).abs() < 1e-6);
    assert!((color[3] - 0.5).abs() < 1e-6);
}

#[test]
fn extraction_sorts_particles_back_to_front() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 0.0,
            burst: 0,
            ..fire()
        },
        [0.0; 3],
    );
    world.insert(
        entity,
        ParticleEmitter {
            particles: vec![
                Particle {
                    position: glam::Vec3::new(0.0, 0.0, 1.0),
                    velocity: glam::Vec3::ZERO,
                    age: 0.0,
                    lifetime: 1.0,
                    size: 1.0,
                },
                Particle {
                    position: glam::Vec3::new(0.0, 0.0, -9.0),
                    velocity: glam::Vec3::ZERO,
                    age: 0.0,
                    lifetime: 1.0,
                    size: 1.0,
                },
            ],
            ..ParticleEmitter::seeded(entity)
        },
    );

    let mut out = Vec::new();
    scene_particles_into(&world, glam::Vec3::ZERO, &mut out);
    assert_eq!(out.len(), 2);
    // 相机在原点、看向 −Z：z = −9 更远，先画。
    assert_eq!(out[0].center[2], -9.0);
    assert_eq!(out[1].center[2], 1.0);
}

#[test]
fn extraction_skips_emitters_without_an_authoring_component() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(
        entity,
        ParticleEmitter {
            particles: vec![Particle {
                position: glam::Vec3::ZERO,
                velocity: glam::Vec3::ZERO,
                age: 0.0,
                lifetime: 1.0,
                size: 1.0,
            }],
            ..ParticleEmitter::seeded(entity)
        },
    );

    let mut out = Vec::new();
    scene_particles_into(&world, glam::Vec3::ZERO, &mut out);
    assert!(out.is_empty(), "没有作者组件的发射器不应出图");
}

#[test]
fn a_huge_dt_is_clamped_to_the_maximum_step() {
    let mut world = World::new();
    let entity = spawn_emitter(
        &mut world,
        ParticleSystem {
            emission_rate: 10.0,
            lifetime: [10.0, 10.0],
            ..fire()
        },
        [0.0; 3],
    );

    // 10 秒的 dt 被截到 0.1 秒 → 只发 1 个（而不是 100 个）。
    particle_system(&mut world, 10.0);
    let particles = particles_of(&world, entity);
    assert_eq!(particles.len(), 1);
    // 新粒子是在积分之后出生的，所以这一帧的年龄还是 0。
    assert_eq!(particles[0].age, 0.0);
}
