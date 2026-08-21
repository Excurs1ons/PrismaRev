//! 音频引擎 — 拥有 Firewheel 上下文并管理声音播放。

use std::num::NonZeroU32;
use std::time::Duration;

use firewheel::core::channel_config::{ChannelCount, NonZeroChannelCount};
use firewheel::core::diff::Notify;
use firewheel::core::node::NodeID;
use firewheel::cpal::{CpalConfig, CpalOutputConfig, CpalStream};
use firewheel::nodes::sampler::*;
use firewheel::*;

use crate::error::AudioError;

/// 解码后的音频数据，可供播放。
#[derive(Clone)]
pub struct AudioData {
    /// 交错排列的 f32 采样（例如立体声为 L,R,L,R）
    pub samples: Vec<f32>,
    /// 采样率，单位 Hz（例如 44100）。
    pub sample_rate: u32,
    /// 声道数（1 = 单声道，2 = 立体声）
    pub channels: u16,
    /// 总持续时间
    pub duration: Duration,
}

/// 控制正在播放的声音的句柄
///
/// 使用 [`AudioEngine`] 的方法来控制播放：
/// `engine.stop(&handle)`, `engine.set_volume(&handle, 0.5)` 等。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlaybackHandle(u64);

impl PlaybackHandle {
    /// 当此句柄由失败的 `play()` 调用产生时返回 `false`
    ///（例如音频为空或图形错误，且永远不会指向真实的声音）
    pub fn is_valid(&self) -> bool {
        self.0 != 0
    }
}

// ---------------------------------------------------------------------------

/// 音频引擎的配置。
#[derive(Clone)]
pub struct AudioConfig {
    /// 首选音频设备名称。`None` = 系统默认。
    pub device_name: Option<String>,
    /// 输出采样率。默认 44100。
    pub sample_rate: u32,
    /// 输出声道数。默认 2 = 立体声。
    pub channels: u16,
    /// 主音量 0.0–1.0。默认 1.0。
    pub master_volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device_name: None,
            sample_rate: 44100,
            channels: 2,
            master_volume: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------

struct ActiveSound {
    handle: PlaybackHandle,
    node_id: NodeID,
    /// 本地副本，用于调用 `sync_*_event()`。
    sampler: SamplerNode,
}

/// 主音频引擎。
///
/// ```ignore
/// let mut engine = AudioEngine::new(AudioConfig::default())?;
/// engine.update(); // 每帧调用
///
/// let snd = engine.play(&my_audio);
/// engine.set_volume(&snd, 0.5);
/// ```
pub struct AudioEngine {
    ctx: FirewheelContext,
    /// 保持存活以确保音频流持续；放置顺序很重要。
    _stream: Option<CpalStream>,
    active: Vec<ActiveSound>,
    next_id: u64,
    master_volume: f32,
    channels: u16,
    /// 打开/重建音频流所用的配置（挂起时保留，恢复时复用）。
    config: AudioConfig,
}

impl AudioEngine {
    /// 创建新的音频引擎并启动音频流。
    ///
    /// 如果音频设备无法打开，会记录警告，但引擎仍会运行（静默模式——游戏不应因音频而崩溃）。
    pub fn new(config: AudioConfig) -> Result<Self, AudioError> {
        let channels = config.channels;

        let ctx = FirewheelContext::new(FirewheelConfig {
            num_graph_outputs: ChannelCount::new(channels as u32).unwrap_or(ChannelCount::STEREO),
            ..Default::default()
        });

        let mut engine = Self {
            ctx,
            _stream: None,
            active: Vec::new(),
            next_id: 1,
            master_volume: config.master_volume,
            channels,
            config,
        };

        // 尝试激活 cpal 音频流（失败仅告警，静默运行——游戏不应因音频而崩溃）。
        engine.arm_stream();
        Ok(engine)
    }

    /// 尝试打开音频设备并注册 cpal 回调。失败仅告警并保持静默。
    fn arm_stream(&mut self) {
        match Self::try_start_stream(&mut self.ctx, &self.config) {
            Ok(stream) => {
                ::log::info!(
                    "Audio stream started ({} Hz, {} ch)",
                    self.config.sample_rate,
                    self.channels
                );
                self._stream = Some(stream);
            }
            Err(e) => {
                ::log::warn!("Audio stream failed to start, running silent: {e}");
                self._stream = None;
            }
        }
    }

    /// 挂起音频输出：交还音频设备（丢弃 cpal 流），但保留 Firewheel 图与所有
    /// 活动播放节点，以便恢复后「续播」而非「重播」。
    ///
    /// 平台背景切换（Android `onPause` / iOS 退后台）时由宿主（`prism-app`）
    /// 调用。Firewheel 0.12 的 [`CpalStream`] 不暴露 pause/play 控制，挂起的
    /// 唯一机制是 drop 流、恢复时重新注册回调——正是「平台强制线程只注册、
    /// 不持有」的语义。流不存在时为空操作。
    pub fn suspend_stream(&mut self) {
        if self._stream.is_none() {
            return;
        }
        ::log::debug!("Audio stream suspended (device released; graph kept)");
        drop(self._stream.take());
    }

    /// 恢复音频输出：若流已被 [`Self::suspend_stream`] 释放，则以挂起前配置
    /// 重新打开设备并注册回调；活动节点/音量/播放位置原样保留。流仍活跃时为
    /// 空操作。
    pub fn resume_stream(&mut self) {
        if self._stream.is_some() {
            return;
        }
        self.arm_stream();
    }

    fn try_start_stream(
        ctx: &mut FirewheelContext,
        config: &AudioConfig,
    ) -> Result<CpalStream, AudioError> {
        let cpal_config = if let Some(ref name) = config.device_name {
            let host = firewheel::cpal::cpal::default_host();
            let host_enum = firewheel::cpal::HostEnumerator { host };
            let device_id = host_enum
                .output_devices()
                .into_iter()
                .find(|d| d.name.as_deref() == Some(name.as_str()))
                .ok_or_else(|| AudioError::DeviceNotFound(name.clone()))?
                .id;

            CpalConfig {
                output: CpalOutputConfig {
                    device_id: Some(device_id),
                    desired_sample_rate: Some(config.sample_rate),
                    ..Default::default()
                },
                input: None,
            }
        } else {
            CpalConfig {
                output: CpalOutputConfig {
                    desired_sample_rate: Some(config.sample_rate),
                    ..Default::default()
                },
                input: None,
            }
        };

        CpalStream::new(ctx, cpal_config).map_err(|e| AudioError::Init(e.to_string()))
    }

    /// 必须每帧调用一次（最好在帧结束时）。
    ///
    /// 移除已播放完成的声音并推进 Firewheel 上下文。
    pub fn update(&mut self) {
        // 清理已播放完成的声音。
        self.active.retain(|s| {
            let alive = self
                .ctx
                .node_info(s.node_id)
                .and_then(|entry| {
                    entry
                        .info
                        .custom_state
                        .as_ref()
                        .and_then(|state| state.downcast_ref::<CurrentProcessorState>())
                })
                .is_some_and(|ps| ps.playback_state == PlaybackState::Playing);

            if !alive {
                let _ = self.ctx.remove_node(s.node_id);
            }
            alive
        });

        let _ = self.ctx.update();
    }

    /// 播放解码后的声音。返回播放控制句柄。
    pub fn play(&mut self, audio: &AudioData) -> PlaybackHandle {
        let id = PlaybackHandle(self.next_id);
        self.next_id += 1;

        let channels = audio.channels.max(1) as usize;
        let num_frames = audio.samples.len() / channels;
        if num_frames == 0 {
            ::log::error!("Cannot play empty audio");
            return id;
        }

        // 将交错格式转换为平面格式，用于 DecodedAudioF32。
        let mut planar: Vec<Vec<f32>> = vec![vec![0.0f32; num_frames]; channels];
        for (i, &sample) in audio.samples.iter().enumerate() {
            let ch = i % channels;
            let frame = i / channels;
            planar[ch][frame] = sample;
        }

        let sample_rate = match NonZeroU32::new(audio.sample_rate) {
            Some(r) => r,
            None => {
                ::log::error!("Invalid sample rate: {}", audio.sample_rate);
                return id;
            }
        };

        // 将样本资源构建为 ArcGc<dyn SampleResource>。
        let decoded_f32 = symphonium::DecodedAudioF32::new(planar, sample_rate, sample_rate);
        let decoded: symphonium::DecodedAudio = decoded_f32.into();
        let resource = firewheel::dyn_symphonium_resource(decoded);

        // 创建带有初始状态的采样器节点。
        let sampler = SamplerNode {
            volume: Volume::from_percent(self.master_volume * 100.0),
            play: Notify::new(true),
            ..Default::default()
        };

        let sampler_config = SamplerConfig {
            channels: if self.channels >= 2 {
                NonZeroChannelCount::STEREO
            } else {
                NonZeroChannelCount::MONO
            },
            ..Default::default()
        };

        let node_id = match self.ctx.add_node(sampler, Some(sampler_config)) {
            Ok(nid) => nid,
            Err(e) => {
                ::log::error!("Failed to add sampler node to graph: {e}");
                return id;
            }
        };

        // 将样本资源加入队列。
        self.ctx
            .queue_event_for(node_id, SamplerNode::set_dyn_sample_event(resource));

        // 连接到图形输出（立体声对）。
        let out = self.ctx.graph_out_node_id();
        if let Err(e) = self.ctx.connect_stereo(node_id, out, false) {
            ::log::error!("Failed to connect sampler to graph output: {e}");
            let _ = self.ctx.remove_node(node_id);
            return id;
        }

        self.active.push(ActiveSound {
            handle: id,
            node_id,
            sampler,
        });

        id
    }

    /// 立即停止声音。
    pub fn stop(&mut self, handle: &PlaybackHandle) {
        if let Some(idx) = self.active.iter().position(|s| s.handle == *handle) {
            let _ = self.ctx.remove_node(self.active[idx].node_id);
            self.active.swap_remove(idx);
        }
    }

    /// 设置正在播放的声音的音量（0.0 = 静音，1.0 = 原始音量）
    pub fn set_volume(&mut self, handle: &PlaybackHandle, volume: f32) {
        let Some(s) = self.active.iter_mut().find(|s| s.handle == *handle) else {
            return;
        };
        let vol = volume.clamp(0.0, 1.0);
        s.sampler.volume = Volume::from_percent(vol * 100.0);
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_volume_event());
    }

    /// 暂停声音（冻结播放位置）。
    pub fn pause(&mut self, handle: &PlaybackHandle) {
        let Some(s) = self.active.iter_mut().find(|s| s.handle == *handle) else {
            return;
        };
        *s.sampler.play = false;
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_play_event());
    }

    /// 恢复暂停的声音
    pub fn resume(&mut self, handle: &PlaybackHandle) {
        let Some(s) = self.active.iter_mut().find(|s| s.handle == *handle) else {
            return;
        };
        *s.sampler.play = true;
        self.ctx
            .queue_event_for(s.node_id, s.sampler.sync_play_event());
    }

    /// 检查声音当前是否正在播放。
    pub fn is_playing(&self, handle: &PlaybackHandle) -> bool {
        let Some(s) = self.active.iter().find(|s| s.handle == *handle) else {
            return false;
        };
        self.ctx
            .node_info(s.node_id)
            .and_then(|entry| {
                entry
                    .info
                    .custom_state
                    .as_ref()
                    .and_then(|state| state.downcast_ref::<CurrentProcessorState>())
            })
            .is_some_and(|ps| ps.playback_state == PlaybackState::Playing)
    }

    /// 立即停止所有正在播放的声音。
    pub fn stop_all(&mut self) {
        for s in self.active.drain(..) {
            let _ = self.ctx.remove_node(s.node_id);
        }
    }

    /// 设置主音量（0.0–1.0）。影响下一步扩展播放的声音。
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume.clamp(0.0, 1.0);
    }

    /// 当前主音量
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// 音频流是否正在运行。
    pub fn is_active(&self) -> bool {
        self._stream.is_some()
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop_all();
        // CpalStream must be dropped before FirewheelContext.
        let stream = self._stream.take();
        drop(stream);
    }
}
