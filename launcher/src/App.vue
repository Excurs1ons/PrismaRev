<script setup lang="ts">
import { ref } from "vue";
import { errorText, launcher } from "./api";

const launching = ref(false);
const error = ref("");

async function play() {
  if (launching.value) return;
  launching.value = true;
  error.value = "";
  try {
    await launcher.launchGame(JSON.stringify({ scene: "intro" }));
  } catch (cause) {
    error.value = errorText(cause);
  } finally {
    launching.value = false;
  }
}
</script>

<template>
  <main class="mobile-launcher">
    <div class="mobile-bg">
      <span class="bg-orb orb1"></span>
      <span class="bg-orb orb2"></span>
      <span class="bg-orb orb3"></span>
      <div class="mobile-grid"></div>
    </div>
    <section class="mobile-content">
      <h1 class="mobile-title">PrismaRev</h1>
      <p class="mobile-subtitle">Vulkan 实时渲染引擎</p>
      <button
        class="mobile-play"
        :class="{ launching }"
        :disabled="launching"
        @click="play"
      >
        <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
          <path d="M8 5v14l11-7z" fill="currentColor" />
        </svg>
        <span>{{ launching ? "启动中…" : "开始游戏" }}</span>
      </button>
      <p v-if="error" class="mobile-error">{{ error }}</p>
      <footer>实时光栅 · PBR · IBL · 调试视图</footer>
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
  background: #07090e;
  color: #e8ecf4;
  overflow: hidden;
}

button {
  font: inherit;
  color: inherit;
}

.mobile-launcher {
  position: relative;
  height: 100vh;
  display: flex;
  align-items: center;
  justify-content: center;
  overflow: hidden;
}

.mobile-bg {
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
}

.orb2 {
  width: 40vmax;
  height: 40vmax;
  right: -12vmax;
  bottom: -14vmax;
  background: radial-gradient(circle at 70% 70%, #b14bff, transparent 70%);
}

.orb3 {
  width: 30vmax;
  height: 30vmax;
  left: 40%;
  top: 30%;
  background: radial-gradient(circle at 50% 50%, #2ad6c8, transparent 70%);
  opacity: 0.35;
}

.mobile-grid {
  position: absolute;
  inset: 0;
  background-image: linear-gradient(rgba(255, 255, 255, 0.04) 1px, transparent 1px),
    linear-gradient(90deg, rgba(255, 255, 255, 0.04) 1px, transparent 1px);
  background-size: 44px 44px;
  -webkit-mask-image: radial-gradient(circle at 50% 50%, black, transparent 75%);
  mask-image: radial-gradient(circle at 50% 50%, black, transparent 75%);
}

.mobile-content {
  position: relative;
  z-index: 1;
  text-align: center;
  padding: 24px;
}

.mobile-title {
  margin: 0;
  font-size: clamp(48px, 11vw, 120px);
  font-weight: 800;
  letter-spacing: -0.03em;
  background: linear-gradient(120deg, #8ab4ff, #c89bff 45%, #5ff0d8);
  -webkit-background-clip: text;
  background-clip: text;
  -webkit-text-fill-color: transparent;
}

.mobile-subtitle {
  margin: 10px 0 38px;
  font-size: clamp(14px, 2.4vw, 20px);
  color: #9aa6c4;
  letter-spacing: 0.18em;
  text-transform: uppercase;
}

.mobile-play {
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
  overflow: hidden;
}

.mobile-play:disabled {
  cursor: progress;
  opacity: 0.85;
}

.mobile-error {
  margin-top: 18px;
  color: #ff8a8a;
  font-size: 14px;
}

.mobile-content footer {
  margin-top: 46px;
  font-size: 12px;
  color: #6b769a;
  letter-spacing: 0.12em;
}
</style>
