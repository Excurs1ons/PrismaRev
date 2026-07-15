<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { invoke } from "@tauri-apps/api/core";
import gsap from "gsap";

const launching = ref(false);
const error = ref("");

async function play() {
  if (launching.value) return;
  launching.value = true;
  error.value = "";
  try {
    await invoke("launch_game");
    // 游戏 Activity 已启动,启动器转入后台;返回时必须可交互,
    // 否则会一直停在"启动中…"且按钮 disabled(表现为无响应)。
    launching.value = false;
  } catch (e) {
    error.value = `${e}`;
    launching.value = false;
  }
}

let tl: gsap.core.Timeline | null = null;
onMounted(() => {
  tl = gsap.timeline({ defaults: { ease: "power3.out" } });
  tl.from(".bg-orb", { scale: 0.6, opacity: 0, duration: 1.6, stagger: 0.2 })
    .from(".title", { y: 40, opacity: 0, duration: 0.9 }, "-=1.0")
    .from(".subtitle", { y: 20, opacity: 0, duration: 0.7 }, "-=0.5")
    .from(".play-btn", { y: 30, opacity: 0, scale: 0.9, duration: 0.7 }, "-=0.4")
    .from(".footer", { opacity: 0, duration: 0.6 }, "-=0.3");
});
onUnmounted(() => {
  tl?.kill();
});
</script>

<template>
  <main class="launcher">
    <div class="bg">
      <span class="bg-orb orb1"></span>
      <span class="bg-orb orb2"></span>
      <span class="bg-orb orb3"></span>
      <div class="grid"></div>
    </div>

    <section class="content">
      <h1 class="title">PrismaRev</h1>
      <p class="subtitle">Vulkan 实时渲染引擎</p>

      <button
        class="play-btn"
        :class="{ launching: launching }"
        :disabled="launching"
        @click="play"
      >
        <span class="play-glow"></span>
        <svg class="play-icon" viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
          <path d="M8 5v14l11-7z" fill="currentColor" />
        </svg>
        <span class="play-label">{{ launching ? "启动中…" : "开始游戏" }}</span>
      </button>

      <p v-if="error" class="error">{{ error }}</p>

      <footer class="footer">
        <span>实时光栅 · PBR · IBL · 调试视图</span>
      </footer>
    </section>
  </main>
</template>

<style>
:root {
  color-scheme: dark;
}
* {
  box-sizing: border-box;
}
html,
body,
#app {
  height: 100%;
  margin: 0;
}
body {
  font-family: "Inter", -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif;
  background: #05060a;
  color: #e8ecf4;
  overflow: hidden;
}

.launcher {
  position: relative;
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
}

.bg {
  position: absolute;
  inset: 0;
  overflow: hidden;
}
.bg-orb {
  position: absolute;
  border-radius: 50%;
  filter: blur(60px);
  opacity: 0.55;
  mix-blend-mode: screen;
}
.orb1 {
  width: 46vmax;
  height: 46vmax;
  left: -10vmax;
  top: -12vmax;
  background: radial-gradient(circle at 30% 30%, #4f7cff, transparent 70%);
  animation: float1 14s ease-in-out infinite;
}
.orb2 {
  width: 40vmax;
  height: 40vmax;
  right: -12vmax;
  bottom: -14vmax;
  background: radial-gradient(circle at 70% 70%, #b14bff, transparent 70%);
  animation: float2 18s ease-in-out infinite;
}
.orb3 {
  width: 30vmax;
  height: 30vmax;
  left: 40%;
  top: 30%;
  background: radial-gradient(circle at 50% 50%, #2ad6c8, transparent 70%);
  opacity: 0.35;
  animation: float3 22s ease-in-out infinite;
}
.grid {
  position: absolute;
  inset: 0;
  background-image: linear-gradient(rgba(255, 255, 255, 0.04) 1px, transparent 1px),
    linear-gradient(90deg, rgba(255, 255, 255, 0.04) 1px, transparent 1px);
  background-size: 44px 44px;
  -webkit-mask-image: radial-gradient(circle at 50% 50%, black, transparent 75%);
  mask-image: radial-gradient(circle at 50% 50%, black, transparent 75%);
}

@keyframes float1 {
  0%,
  100% {
    transform: translate(0, 0);
  }
  50% {
    transform: translate(6vmax, 4vmax);
  }
}
@keyframes float2 {
  0%,
  100% {
    transform: translate(0, 0);
  }
  50% {
    transform: translate(-5vmax, -3vmax);
  }
}
@keyframes float3 {
  0%,
  100% {
    transform: translate(0, 0) scale(1);
  }
  50% {
    transform: translate(-3vmax, 5vmax) scale(1.1);
  }
}

.content {
  position: relative;
  z-index: 1;
  text-align: center;
  padding: 24px;
}
.title {
  margin: 0;
  font-size: clamp(48px, 11vw, 120px);
  font-weight: 800;
  letter-spacing: -0.03em;
  background: linear-gradient(120deg, #8ab4ff, #c89bff 45%, #5ff0d8);
  -webkit-background-clip: text;
  background-clip: text;
  -webkit-text-fill-color: transparent;
  filter: drop-shadow(0 6px 30px rgba(120, 140, 255, 0.35));
}
.subtitle {
  margin: 10px 0 38px;
  font-size: clamp(14px, 2.4vw, 20px);
  color: #9aa6c4;
  letter-spacing: 0.18em;
  text-transform: uppercase;
}

.play-btn {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 12px;
  padding: 16px 40px;
  border: none;
  border-radius: 999px;
  font-size: 20px;
  font-weight: 700;
  color: #06121f;
  cursor: pointer;
  background: linear-gradient(120deg, #8ab4ff, #5ff0d8);
  box-shadow: 0 10px 40px rgba(90, 200, 255, 0.45);
  transition: transform 0.15s ease, box-shadow 0.25s ease;
  overflow: hidden;
}
.play-btn:hover {
  transform: translateY(-2px) scale(1.03);
  box-shadow: 0 14px 54px rgba(90, 200, 255, 0.6);
}
.play-btn:active {
  transform: scale(0.97);
}
.play-btn.launching {
  cursor: progress;
  opacity: 0.85;
}
.play-glow {
  position: absolute;
  inset: 0;
  background: linear-gradient(120deg, transparent, rgba(255, 255, 255, 0.6), transparent);
  transform: translateX(-120%);
  animation: sheen 2.4s ease-in-out infinite;
}
.play-btn.launching .play-glow {
  animation-duration: 1s;
}
@keyframes sheen {
  0% {
    transform: translateX(-120%);
  }
  60%,
  100% {
    transform: translateX(120%);
  }
}
.play-icon {
  filter: drop-shadow(0 1px 2px rgba(0, 0, 0, 0.3));
}
.play-label {
  letter-spacing: 0.04em;
}

.error {
  margin-top: 18px;
  color: #ff8a8a;
  font-size: 14px;
}
.footer {
  margin-top: 46px;
  font-size: 12px;
  color: #6b769a;
  letter-spacing: 0.12em;
}
</style>
