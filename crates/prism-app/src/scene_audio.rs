//! 场景音频：把 ECS 里的 [`AudioSource`] 接到混音器上。
//!
//! 分层与 `MeshRenderer → MeshRef` 一致：`AudioSource` 只写**作者态**
//! （片段路径 / 音量 / 是否循环 / 距离衰减），本模块负责
//!
//! 1. 按路径从 `assets/` 找文件，交给后台解码线程（`audio_decode_runner`）；
//! 2. 解码结果按路径缓存，同一个片段只解码一次；
//! 3. 每个实体一条「声部」：`play_on_awake` + 片段就绪就播放，之后每帧同步音量、
//!    循环重播、实体消失时停止。
//!
//! 混音器通过 [`AudioSink`] 抽象，因此这套状态机可以在没有声卡的环境里单测
//! （见文件末尾的 `tests`）。
//!
//! # 现状与边界
//!
//! - **只有距离衰减**：`spatial_blend` 把衰减曲线与 2D 满音量混合；左右声像
//!   （`pan`）还没有，混音器 API 里也还没有 `set_volume_pan`。
//! - 只读**松散文件**：`assets/**` 里的音频；`.pak` 内的音频还没接。
//! - 片段的采样率/声道由解码器给；混音器负责重采样。
//! - 不监听文件变化：改音频后需要重启或重新加载场景（资产热重载只处理 `.pak`）。

use std::collections::HashMap;

use flume::{Receiver, Sender};
use prism_audio::AudioData;
use prism_engine::ecs::{Entity, World};
use prism_engine::scene::components::{AudioListener, AudioSource, Camera, LocalToWorld};

use crate::audio_decode_runner::{DecodeRequest, DecodeResult};

/// 混音器的最小接口。
///
/// 实现者维护 `voice`（不透明 id）到真实播放句柄的映射；`0` 表示「播放失败」。
pub trait AudioSink {
    /// 开始播放，返回声部 id（`0` = 失败）。
    fn play(&mut self, clip: &AudioData) -> u64;
    fn stop(&mut self, voice: u64);
    fn set_volume(&mut self, voice: u64, volume: f32);
    fn is_playing(&self, voice: u64) -> bool;
}

/// 一个实体的声部。
#[derive(Debug, Clone)]
struct Voice {
    /// 混音器声部 id。
    id: u64,
    /// 已经下发给混音器的音量（避免每帧重复排队事件）。
    volume: f32,
    /// 音频线程是否真的开始播了（`play()` 刚返回时它还是 `false`）。
    started: bool,
    /// 声部对应的片段路径（路径变了要重播）。
    path: String,
}

/// 场景音频驱动。由宿主（`App`）每帧调用一次。
#[derive(Default)]
pub struct SceneAudio {
    /// 已解码片段（路径 → 采样）。
    clips: HashMap<String, AudioData>,
    /// 解码失败或正在解码的路径（避免同一帧重复入队）。
    pending: HashMap<String, u64>,
    /// 请求 id → 路径，用于把异步结果对回片段。
    requests: HashMap<u64, String>,
    next_request: u64,
    voices: HashMap<Entity, Voice>,
}

impl SceneAudio {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已缓存的片段数（诊断 / 测试）。
    pub fn cached_clips(&self) -> usize {
        self.clips.len()
    }

    /// 正在播放的声部数（诊断 / 测试）。
    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// 直接注入一段已解码的片段（测试 / 代码里预载声音）。
    pub fn insert_clip(&mut self, path: &str, clip: AudioData) {
        self.add_clip(path, clip);
    }

    /// 收下解码线程的结果：成功则入缓存。
    pub fn receive(&mut self, result: DecodeResult) {
        match result {
            DecodeResult::Decoded { request_id, data } => {
                let Some(path) = self.requests.remove(&request_id) else {
                    return;
                };
                self.pending.remove(&path);
                self.add_clip(&path, data);
            }
            DecodeResult::Error { request_id, .. } => {
                if let Some(path) = self.requests.remove(&request_id) {
                    // 失败不缓存：下次还会重试（文件可能稍后才出现）。
                    self.pending.remove(&path);
                }
            }
        }
    }

    /// 停止全部声部（场景卸载 / 关闭时调用）。
    pub fn stop_all(&mut self, sink: &mut dyn AudioSink) {
        for (_, voice) in self.voices.drain() {
            sink.stop(voice.id);
        }
    }

    /// 每帧同步：播放新声部、更新音量、循环重播、回收消失的实体。
    ///
    /// `requests` 是解码线程的输入通道（`None` = 没有解码线程，只播放已缓存片段）。
    pub fn update(
        &mut self,
        world: &World,
        sink: &mut dyn AudioSink,
        requests: Option<&Sender<DecodeRequest>>,
    ) {
        let (listener_position, listener_gain) = listener(world);
        let mut live: Vec<Entity> = Vec::new();

        let sources: Vec<(Entity, AudioSource)> = world
            .query::<AudioSource>()
            .map(|(entity, source)| (entity, source.clone()))
            .collect();

        for (entity, source) in sources {
            live.push(entity);
            if !source.has_sound() || !source.play_on_awake {
                if let Some(voice) = self.voices.remove(&entity) {
                    sink.stop(voice.id);
                }
                continue;
            }

            let volume = match (world_position(world, entity), listener_position) {
                (Some(position), Some(listener)) => {
                    source.volume_at(distance(position, listener)) * listener_gain
                }
                _ => source.volume_at(0.0) * listener_gain,
            }
            .clamp(0.0, 1.0);

            if !self.clips.contains_key(&source.sound_path) {
                self.request(&source.sound_path, requests);
                continue;
            }

            // 路径变了：旧声部先停。
            if let Some(voice) = self.voices.get(&entity) {
                if voice.path != source.sound_path {
                    let voice = self.voices.remove(&entity).expect("checked above");
                    sink.stop(voice.id);
                }
            }

            match self.voices.get_mut(&entity) {
                None => {
                    let Some(clip) = self.clips.get(&source.sound_path) else {
                        continue;
                    };
                    let path = source.sound_path.clone();
                    let id = sink.play(clip);
                    if id == 0 {
                        continue;
                    }
                    sink.set_volume(id, volume);
                    self.voices.insert(
                        entity,
                        Voice {
                            id,
                            volume,
                            started: false,
                            path,
                        },
                    );
                }
                Some(voice) => {
                    if sink.is_playing(voice.id) {
                        voice.started = true;
                    } else if voice.started {
                        // 播完了：循环的重来，否则回收。
                        if !source.looped {
                            let voice = self.voices.remove(&entity).expect("checked above");
                            sink.stop(voice.id);
                            continue;
                        }
                        let Some(clip) = self.clips.get(&source.sound_path) else {
                            continue;
                        };
                        let path = source.sound_path.clone();
                        let id = sink.play(clip);
                        if id == 0 {
                            continue;
                        }
                        sink.set_volume(id, volume);
                        self.voices.insert(
                            entity,
                            Voice {
                                id,
                                volume,
                                started: false,
                                path,
                            },
                        );
                        continue;
                    }
                    if (voice.volume - volume).abs() > 0.001 {
                        voice.volume = volume;
                        sink.set_volume(voice.id, volume);
                    }
                }
            }
        }

        // 实体被销毁 / 组件被移除：停掉对应声部。
        let stale: Vec<Entity> = self
            .voices
            .keys()
            .filter(|entity| !live.contains(entity))
            .copied()
            .collect();
        for entity in stale {
            if let Some(voice) = self.voices.remove(&entity) {
                sink.stop(voice.id);
            }
        }
    }

    fn add_clip(&mut self, path: &str, clip: AudioData) {
        self.clips.insert(path.to_string(), clip);
    }

    fn request(&mut self, path: &str, requests: Option<&Sender<DecodeRequest>>) {
        if self.pending.contains_key(path) {
            return;
        }
        let Some(requests) = requests else {
            return;
        };
        let Some(file) = prism_engine::asset_paths::asset_path(path) else {
            log::warn!("audio clip not found in assets/: {path}");
            return;
        };
        self.next_request += 1;
        let request_id = self.next_request;
        self.pending.insert(path.to_string(), request_id);
        self.requests.insert(request_id, path.to_string());
        if let Err(error) = requests.send(DecodeRequest::DecodeFile {
            path: file.to_string_lossy().into_owned(),
            request_id,
        }) {
            log::warn!("audio decode thread is gone: {error}");
            self.pending.remove(path);
            self.requests.remove(&request_id);
        }
    }
}

/// 听者位置与增益：优先 `AudioListener` 实体，否则退回第一个相机，再否则原点。
fn listener(world: &World) -> (Option<[f32; 3]>, f32) {
    let gain = |entity: Entity, fallback: f32| {
        world
            .get::<AudioListener>(entity)
            .map(|listener| listener.gain.clamp(0.0, 1.0))
            .unwrap_or(fallback)
    };
    if let Some((entity, _)) = world.query::<AudioListener>().next() {
        return (world_position(world, entity), gain(entity, 1.0));
    }
    if let Some((entity, _)) = world.query::<Camera>().next() {
        return (world_position(world, entity), 1.0);
    }
    (Some([0.0; 3]), 1.0)
}

fn world_position(world: &World, entity: Entity) -> Option<[f32; 3]> {
    world
        .get::<LocalToWorld>(entity)
        .map(|world| world.0.w_axis.truncate().to_array())
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// 把 [`AudioSink`] 的 `u64` 声部映射到真实 [`prism_audio::PlaybackHandle`]。
#[derive(Debug, Default)]
pub struct VoiceHandles {
    handles: HashMap<u64, prism_audio::PlaybackHandle>,
    next: u64,
}

impl AudioSink for (&mut prism_audio::AudioEngine, &mut VoiceHandles) {
    fn play(&mut self, clip: &AudioData) -> u64 {
        let handle = self.0.play(clip);
        if !handle.is_valid() {
            return 0;
        }
        self.1.next += 1;
        let id = self.1.next;
        self.1.handles.insert(id, handle);
        id
    }

    fn stop(&mut self, voice: u64) {
        if let Some(handle) = self.1.handles.remove(&voice) {
            self.0.stop(&handle);
        }
    }

    fn set_volume(&mut self, voice: u64, volume: f32) {
        if let Some(handle) = self.1.handles.get(&voice) {
            self.0.set_volume(handle, volume);
        }
    }

    fn is_playing(&self, voice: u64) -> bool {
        self.1
            .handles
            .get(&voice)
            .map(|handle| self.0.is_playing(handle))
            .unwrap_or(false)
    }
}

/// 解码结果分发：宿主每帧把 `DecodeResult` 交给 [`SceneAudio::receive`]。
pub fn drain_decoded(scene_audio: &mut SceneAudio, results: &Receiver<DecodeResult>) -> usize {
    let mut drained = 0;
    while let Ok(result) = results.try_recv() {
        scene_audio.receive(result);
        drained += 1;
    }
    drained
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;
    use std::time::Duration;

    /// 录音式假混音器：记录调用，`is_playing` 可以手动改。
    #[derive(Default)]
    struct FakeSink {
        next: u64,
        ends: HashMap<u64, &'static str>,
        playing: HashMap<u64, bool>,
        volumes: HashMap<u64, f32>,
        stopped: Vec<u64>,
        played: usize,
    }

    impl FakeSink {
        fn finish(&mut self, voice: u64) {
            self.playing.insert(voice, false);
        }
        fn volumes(&self) -> Vec<f32> {
            let mut values: Vec<(u64, f32)> = self.volumes.iter().map(|(k, v)| (*k, *v)).collect();
            values.sort_by(|a, b| a.0.cmp(&b.0));
            values.into_iter().map(|(_, volume)| volume).collect()
        }
    }

    impl AudioSink for FakeSink {
        fn play(&mut self, clip: &AudioData) -> u64 {
            assert!(!clip.samples.is_empty(), "空片段不该开始播放");
            self.next += 1;
            let id = self.next;
            self.played += 1;
            self.playing.insert(id, true);
            self.ends.insert(id, "clip");
            id
        }
        fn stop(&mut self, voice: u64) {
            self.playing.insert(voice, false);
            self.stopped.push(voice);
        }
        fn set_volume(&mut self, voice: u64, volume: f32) {
            self.volumes.insert(voice, volume);
        }
        fn is_playing(&self, voice: u64) -> bool {
            self.playing.get(&voice).copied().unwrap_or(false)
        }
    }

    fn clip() -> AudioData {
        AudioData {
            samples: vec![0.0; 16],
            sample_rate: 44_100,
            channels: 1,
            duration: Duration::from_secs(1),
        }
    }

    fn place(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            LocalToWorld(Mat4::from_translation(glam::Vec3::from_array(position))),
        );
        entity
    }

    fn source(world: &mut World, entity: Entity, source: AudioSource) {
        world.insert(entity, source);
    }

    fn plain(path: &str) -> AudioSource {
        AudioSource {
            sound_path: path.into(),
            ..AudioSource::default()
        }
    }

    #[test]
    fn a_clip_plays_once_and_follows_the_distance_volume() {
        let mut world = World::new();
        place(&mut world, [0.0; 3]); // 默认监听者 = 世界原点
        let speaker = place(&mut world, [0.0, 0.0, 5.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                spatial_blend: 1.0,
                min_distance: 0.0,
                max_distance: 10.0,
                volume: 1.0,
                ..plain("audio/beep.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();

        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1, "片段只播一次");
        assert_eq!(scene.active_voices(), 1);
        assert!((sink.volumes()[0] - 0.5).abs() < 1e-6, "距离 5/10 → 半音量");

        // 再靠近一点：音量更新，但不重新播放。
        world.insert(
            speaker,
            LocalToWorld(Mat4::from_translation(glam::Vec3::new(0.0, 0.0, 2.5))),
        );
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 1);
        assert!((sink.volumes()[0] - 0.75).abs() < 1e-6);
    }

    #[test]
    fn a_2d_source_ignores_the_distance() {
        let mut world = World::new();
        let speaker = place(&mut world, [0.0, 0.0, 500.0]);
        source(
            &mut world,
            speaker,
            AudioSource {
                volume: 0.4,
                ..plain("audio/ui.ogg")
            },
        );

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/ui.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn the_listener_gain_scales_every_source() {
        let mut world = World::new();
        let listener = place(&mut world, [0.0; 3]);
        world.insert(listener, AudioListener { gain: 0.5 });
        let speaker = place(&mut world, [0.0; 3]);
        source(&mut world, speaker, plain("audio/beep.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/beep.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert!((sink.volumes()[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_looped_source_restarts_but_a_one_shot_is_released() {
        let mut world = World::new();
        let looped = place(&mut world, [0.0; 3]);
        source(
            &mut world,
            looped,
            AudioSource {
                looped: true,
                ..plain("audio/loop.ogg")
            },
        );
        let one_shot = place(&mut world, [0.0; 3]);
        source(&mut world, one_shot, plain("audio/hit.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/loop.ogg", clip());
        scene.insert_clip("audio/hit.ogg", clip());
        let mut sink = FakeSink::default();

        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 2);
        assert_eq!(scene.active_voices(), 2);

        // 音频线程开始播放后才允许判定「播完」。
        scene.update(&world, &mut sink, None);
        for voice in 1..=2 {
            sink.finish(voice);
        }
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 3, "只有循环的那个重播");
        assert_eq!(scene.active_voices(), 1);
        assert_eq!(sink.stopped.len(), 1, "一次性音效被回收");
    }

    #[test]
    fn clearing_the_component_or_despawning_stops_the_voice() {
        let mut world = World::new();
        let a = place(&mut world, [0.0; 3]);
        source(&mut world, a, plain("audio/a.ogg"));
        let b = place(&mut world, [0.0; 3]);
        source(&mut world, b, plain("audio/a.ogg"));

        let mut scene = SceneAudio::new();
        scene.insert_clip("audio/a.ogg", clip());
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert_eq!(scene.active_voices(), 2);

        // 一个把路径清空，另一个直接销毁。
        world.insert(
            a,
            AudioSource {
                sound_path: String::new(),
                ..AudioSource::default()
            },
        );
        world.despawn(b);
        scene.update(&world, &mut sink, None);
        assert_eq!(scene.active_voices(), 0);
        assert_eq!(sink.stopped.len(), 2);
    }

    #[test]
    fn a_missing_clip_does_not_block_the_voice_slot() {
        let mut world = World::new();
        let speaker = place(&mut world, [0.0; 3]);
        source(&mut world, speaker, plain("audio/not-cached.ogg"));

        let mut scene = SceneAudio::new();
        let mut sink = FakeSink::default();
        scene.update(&world, &mut sink, None);
        assert_eq!(sink.played, 0);
        assert_eq!(scene.active_voices(), 0, "没有片段就不占声部");
    }
}
