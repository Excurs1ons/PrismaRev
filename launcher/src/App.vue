<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { errorText, hub, type HubEnvironment, type ProjectEntry } from "./api";

type View = "projects" | "editor";
type Notice = { kind: "ok" | "error"; text: string };

const env = ref<HubEnvironment | null>(null);
const ready = ref(false);
const bootError = ref("");
const view = ref<View>("projects");
const projects = ref<ProjectEntry[]>([]);
const selectedPath = ref<string | null>(null);
const filter = ref("");
const busy = ref<string | null>(null);
const notice = ref<Notice | null>(null);

const showNew = ref(false);
const newName = ref("");
const newLocation = ref("");
const creating = ref(false);

const launching = ref(false);
const mobileError = ref("");

let noticeTimer: number | undefined;
let pollTimer: number | undefined;

const isMobile = computed(() => env.value?.platform === "mobile");
const editor = computed(() => env.value?.editor ?? null);
const selected = computed(
  () => projects.value.find((project) => project.path === selectedPath.value) ?? null,
);
const filteredProjects = computed(() => {
  const query = filter.value.trim().toLowerCase();
  if (!query) return projects.value;
  return projects.value.filter(
    (project) =>
      project.name.toLowerCase().includes(query) ||
      project.path.toLowerCase().includes(query),
  );
});
const newProjectPath = computed(() => {
  const location = newLocation.value.trim().replace(/[\\/]+$/, "");
  const name = newName.value.trim();
  if (!location || !name) return "";
  return `${location}/${name}`;
});
const canCreate = computed(
  () => Boolean(newName.value.trim() && newLocation.value.trim()) && !creating.value,
);

function setNotice(kind: Notice["kind"], text: string) {
  notice.value = { kind, text };
  if (noticeTimer !== undefined) window.clearTimeout(noticeTimer);
  noticeTimer = window.setTimeout(() => {
    notice.value = null;
  }, 4200);
}

async function refresh(silent = false) {
  try {
    const next = await hub.projects();
    projects.value = next;
    if (!next.some((project) => project.path === selectedPath.value)) {
      selectedPath.value = next[0]?.path ?? null;
    }
  } catch (error) {
    if (!silent) setNotice("error", errorText(error));
  }
}

async function openProject(project: ProjectEntry) {
  if (!project.valid || project.running || busy.value) return;
  busy.value = project.path;
  try {
    const updated = await hub.openProject(project.path);
    selectedPath.value = updated.path;
    setNotice("ok", `已启动 ${updated.name} 的编辑器`);
    await refresh(true);
  } catch (error) {
    setNotice("error", errorText(error));
  } finally {
    busy.value = null;
  }
}

async function addProject() {
  try {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "选择 PrismaRev 工程目录（包含 assets/scenes）",
    });
    if (typeof picked !== "string") return;
    busy.value = picked;
    const entry = await hub.addProject(picked);
    view.value = "projects";
    selectedPath.value = entry.path;
    await refresh(true);
    setNotice("ok", `已添加工程 ${entry.name}`);
  } catch (error) {
    setNotice("error", errorText(error));
  } finally {
    busy.value = null;
  }
}

async function removeSelected() {
  const project = selected.value;
  if (!project) return;
  if (!window.confirm(`从 Hub 移除「${project.name}」？不会删除磁盘文件。`)) return;
  try {
    projects.value = await hub.removeProject(project.path);
    if (!projects.value.some((item) => item.path === selectedPath.value)) {
      selectedPath.value = projects.value[0]?.path ?? null;
    }
    setNotice("ok", `已从列表移除 ${project.name}`);
  } catch (error) {
    setNotice("error", errorText(error));
  }
}

async function reveal(project: ProjectEntry) {
  try {
    await hub.revealProject(project.path);
  } catch (error) {
    setNotice("error", errorText(error));
  }
}

function openNewProject() {
  newName.value = "";
  newLocation.value = env.value?.repoRoot ? `${env.value.repoRoot}/projects` : "";
  showNew.value = true;
}

async function chooseLocation() {
  try {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "选择工程父目录",
    });
    if (typeof picked === "string") newLocation.value = picked;
  } catch (error) {
    setNotice("error", errorText(error));
  }
}

async function createProject() {
  if (!canCreate.value) return;
  creating.value = true;
  try {
    const entry = await hub.createProject(newName.value.trim(), newLocation.value.trim());
    showNew.value = false;
    view.value = "projects";
    selectedPath.value = entry.path;
    await refresh(true);
    setNotice("ok", `已创建工程 ${entry.name}`);
  } catch (error) {
    setNotice("error", errorText(error));
  } finally {
    creating.value = false;
  }
}

async function refreshEditor() {
  try {
    const info = await hub.editorInfo();
    if (env.value) env.value = { ...env.value, editor: info };
    setNotice(
      info.found ? "ok" : "error",
      info.found ? `已找到编辑器 ${info.version}` : "仍未找到编辑器",
    );
  } catch (error) {
    setNotice("error", errorText(error));
  }
}

async function playMobile() {
  if (launching.value) return;
  launching.value = true;
  mobileError.value = "";
  try {
    await hub.launchGame(JSON.stringify({ scene: "intro" }));
  } catch (error) {
    mobileError.value = errorText(error);
  } finally {
    launching.value = false;
  }
}

function relativeTime(seconds: number | null): string {
  if (!seconds) return "从未打开";
  const diff = Math.floor(Date.now() / 1000 - seconds);
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)} 天前`;
  return new Date(seconds * 1000).toLocaleDateString();
}

function prettyPath(path: string): string {
  return path.replace(/^\\\\\?\\/, "");
}

onMounted(async () => {
  try {
    env.value = await hub.environment();
    if (isMobile.value) return;
    await refresh();
    pollTimer = window.setInterval(() => void refresh(true), 2500);
  } catch (error) {
    bootError.value = errorText(error);
  } finally {
    ready.value = true;
  }
});

onUnmounted(() => {
  if (pollTimer !== undefined) window.clearInterval(pollTimer);
  if (noticeTimer !== undefined) window.clearTimeout(noticeTimer);
});
</script>

<template>
  <div v-if="!ready" class="boot">
    <div class="boot-mark">P</div>
    <span>正在读取工程列表…</span>
  </div>

  <main v-else-if="bootError" class="boot error-boot">
    <div class="boot-mark">!</div>
    <strong>Hub 启动失败</strong>
    <p>{{ bootError }}</p>
  </main>

  <main v-else-if="isMobile" class="mobile-launcher">
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
        @click="playMobile"
      >
        <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
          <path d="M8 5v14l11-7z" fill="currentColor" />
        </svg>
        <span>{{ launching ? "启动中…" : "开始游戏" }}</span>
      </button>
      <p v-if="mobileError" class="mobile-error">{{ mobileError }}</p>
      <footer>实时光栅 · PBR · IBL · 调试视图</footer>
    </section>
  </main>

  <main v-else class="hub">
    <aside class="sidebar">
      <div class="brand">
        <div class="brand-mark">P</div>
        <div class="brand-text">
          <strong>PrismaRev</strong>
          <span>Hub</span>
        </div>
      </div>

      <nav class="nav">
        <button
          class="nav-item"
          :class="{ active: view === 'projects' }"
          @click="view = 'projects'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path
              d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10Z"
            />
          </svg>
          <span>工程</span>
          <em>{{ projects.length }}</em>
        </button>
        <button
          class="nav-item"
          :class="{ active: view === 'editor' }"
          @click="view = 'editor'"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M4 5h16v11H4z" />
            <path d="M9 20h6M12 16v4" />
          </svg>
          <span>编辑器</span>
        </button>
      </nav>

      <button class="sidebar-foot" :class="{ ok: editor?.found }" @click="view = 'editor'">
        <span class="status-dot"></span>
        <span class="foot-text">
          <strong>{{ editor?.found ? "编辑器就绪" : "未找到编辑器" }}</strong>
          <span>{{ editor?.found ? `v${editor.version}` : "点击查看" }}</span>
        </span>
      </button>
    </aside>

    <section class="workspace">
      <header class="topbar">
        <div class="heading">
          <h1>{{ view === "projects" ? "工程" : "编辑器" }}</h1>
          <p v-if="view === 'projects'">
            {{ projects.length }} 个工程 · 双击列表项直接打开
          </p>
          <p v-else>Hub 启动编辑器时使用的安装</p>
        </div>

        <div v-if="view === 'projects'" class="actions">
          <label class="search">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="11" cy="11" r="7" />
              <path d="m16 16 4 4" />
            </svg>
            <input v-model="filter" placeholder="搜索工程" spellcheck="false" />
          </label>
          <button class="btn" @click="addProject">添加</button>
          <button class="btn primary" @click="openNewProject">新建工程</button>
        </div>
        <div v-else class="actions">
          <button class="btn" @click="refreshEditor">重新检测</button>
        </div>
      </header>

      <div v-if="notice" class="notice" :class="notice.kind">{{ notice.text }}</div>

      <div v-if="view === 'projects'" class="projects">
        <div class="list">
          <div
            v-for="project in filteredProjects"
            :key="project.path"
            class="project-row"
            :class="{ selected: project.path === selectedPath }"
            tabindex="0"
            @click="selectedPath = project.path"
            @dblclick="openProject(project)"
            @keydown.enter="openProject(project)"
          >
            <div class="project-icon">
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10Z"
                />
              </svg>
            </div>
            <div class="project-main">
              <div class="project-title">
                <strong>{{ project.name }}</strong>
                <span v-if="project.running" class="badge running">运行中</span>
                <span v-if="!project.exists" class="badge missing">丢失</span>
                <span v-else-if="!project.valid" class="badge warn">无效</span>
              </div>
              <span class="project-path mono">{{ prettyPath(project.path) }}</span>
            </div>
            <div class="project-side">
              <time>{{ relativeTime(project.lastOpened) }}</time>
              <span>{{ project.sceneCount }} 个场景</span>
            </div>
          </div>

          <div v-if="!projects.length" class="empty-list">
            <div class="empty-art">
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10Z"
                />
              </svg>
            </div>
            <strong>还没有工程</strong>
            <p>新建一个工程，或把已有的 PrismaRev 工程目录添加到 Hub。</p>
            <div class="empty-actions">
              <button class="btn" @click="addProject">添加已有工程</button>
              <button class="btn primary" @click="openNewProject">新建工程</button>
            </div>
          </div>

          <div v-else-if="!filteredProjects.length" class="empty-list compact">
            <strong>没有匹配「{{ filter }}」的工程</strong>
            <button class="link-btn" @click="filter = ''">清除搜索</button>
          </div>
        </div>

        <aside class="detail">
          <template v-if="selected">
            <div class="detail-head">
              <div class="project-icon large">
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path
                    d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5v-10Z"
                  />
                </svg>
              </div>
              <div class="detail-title">
                <h2>{{ selected.name }}</h2>
                <p class="mono">{{ prettyPath(selected.path) }}</p>
              </div>
            </div>

            <div class="detail-actions">
              <button
                class="btn primary block"
                :disabled="busy === selected.path || !selected.valid || selected.running"
                @click="openProject(selected)"
              >
                {{
                  selected.running
                    ? "已在编辑器中打开"
                    : busy === selected.path
                      ? "正在启动…"
                      : "打开工程"
                }}
              </button>
              <button class="btn block" @click="reveal(selected)">
                在文件管理器中显示
              </button>
            </div>

            <div v-if="!selected.exists" class="callout danger">工程目录已丢失</div>
            <div v-else-if="!selected.valid" class="callout danger">
              缺少 assets/scenes，无法作为工程打开
            </div>

            <dl class="meta">
              <div>
                <dt>场景</dt>
                <dd>{{ selected.sceneCount }} 个</dd>
              </div>
              <div>
                <dt>游戏 crate</dt>
                <dd>{{ selected.hasGameCrate ? "已找到" : "未找到" }}</dd>
              </div>
              <div>
                <dt>最近打开</dt>
                <dd>{{ relativeTime(selected.lastOpened) }}</dd>
              </div>
              <div>
                <dt>添加时间</dt>
                <dd>{{ relativeTime(selected.addedAt) }}</dd>
              </div>
            </dl>

            <button class="btn danger ghost block" @click="removeSelected">
              从列表移除
            </button>
          </template>

          <div v-else class="detail-empty">
            <strong>未选择工程</strong>
            <p>在左侧选择一个工程查看详情。</p>
          </div>
        </aside>
      </div>

      <div v-else class="editor-view">
        <div class="editor-card">
          <div class="editor-head">
            <span class="status-dot" :class="{ ok: editor?.found }"></span>
            <div class="editor-title">
              <h2>{{ editor?.found ? "PrismaRev Editor" : "未找到编辑器" }}</h2>
              <p>
                {{
                  editor?.found
                    ? `版本 ${editor.version}`
                    : "构建 editor/ 后重新检测，或用 PRISMREV_EDITOR_BIN 指定路径"
                }}
              </p>
            </div>
            <button class="btn" @click="refreshEditor">重新检测</button>
          </div>

          <dl class="meta">
            <div>
              <dt>二进制</dt>
              <dd class="mono">{{ editor?.path ?? "—" }}</dd>
            </div>
            <div>
              <dt>仓库根</dt>
              <dd class="mono">{{ env?.repoRoot ?? "未检测到（打包安装）" }}</dd>
            </div>
          </dl>

          <pre v-if="editor?.hint" class="hint mono">{{ editor.hint }}</pre>
          <p class="hint">
            Hub 以工程目录为工作目录启动编辑器；编辑器据此向上查找
            <code>assets/scenes</code>。同一工程不会重复启动。
          </p>
        </div>
      </div>
    </section>
  </main>

  <div v-if="showNew" class="modal-backdrop" @click.self="showNew = false">
    <div class="modal">
      <header class="modal-head">
        <div>
          <h2>新建工程</h2>
          <p>创建一个可以直接在编辑器中打开的资源工程。</p>
        </div>
        <button class="icon-btn" aria-label="关闭" @click="showNew = false">×</button>
      </header>

      <label class="field">
        <span>工程名称</span>
        <input
          v-model="newName"
          placeholder="MyGame"
          spellcheck="false"
          autofocus
          @keydown.enter="createProject"
        />
      </label>

      <label class="field">
        <span>位置</span>
        <div class="field-row">
          <input v-model="newLocation" placeholder="选择工程父目录" spellcheck="false" />
          <button class="btn" @click="chooseLocation">浏览…</button>
        </div>
      </label>

      <p v-if="newProjectPath" class="path-preview mono">{{ newProjectPath }}</p>

      <div class="template">
        <div class="template-card selected">
          <strong>空工程</strong>
          <span>默认场景（相机 + 平行光）与场景清单</span>
        </div>
      </div>

      <footer class="modal-foot">
        <button class="btn" @click="showNew = false">取消</button>
        <button class="btn primary" :disabled="!canCreate" @click="createProject">
          {{ creating ? "正在创建…" : "创建工程" }}
        </button>
      </footer>
    </div>
  </div>
</template>

<style>
:root {
  color-scheme: dark;
  --bg: #07090e;
  --panel: #0d1119;
  --panel-2: #131925;
  --panel-3: #1a2130;
  --line: #232b3b;
  --text: #e8ecf4;
  --muted: #8a96ae;
  --muted-2: #616d85;
  --accent: #7aa2ff;
  --accent-strong: #5f8dff;
  --teal: #5ff0d8;
  --danger: #ff8080;
  --ok: #5ad9a4;
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
  background: var(--bg);
  color: var(--text);
  overflow: hidden;
}

button,
input {
  font: inherit;
  color: inherit;
}

.mono {
  font-family: "JetBrains Mono", "Cascadia Code", "SFMono-Regular", Consolas, monospace;
}

/* ---------------------------------------------------------------- boot */

.boot {
  height: 100vh;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  color: var(--muted);
  font-size: 14px;
}

.boot-mark {
  width: 46px;
  height: 46px;
  display: grid;
  place-items: center;
  border-radius: 14px;
  font-weight: 800;
  font-size: 22px;
  color: #06121f;
  background: linear-gradient(135deg, var(--accent), var(--teal));
  box-shadow: 0 10px 34px rgba(95, 141, 255, 0.35);
}

.error-boot strong {
  color: var(--danger);
}

.error-boot p {
  max-width: 520px;
  text-align: center;
  white-space: pre-wrap;
}

/* ---------------------------------------------------------------- hub shell */

.hub {
  height: 100vh;
  display: flex;
  background:
    radial-gradient(1200px 700px at 70% -10%, rgba(95, 141, 255, 0.12), transparent 60%),
    var(--bg);
}

.sidebar {
  width: 216px;
  flex: none;
  display: flex;
  flex-direction: column;
  gap: 18px;
  padding: 20px 14px 16px;
  background: rgba(10, 13, 20, 0.86);
  border-right: 1px solid var(--line);
}

.brand {
  display: flex;
  align-items: center;
  gap: 11px;
  padding: 0 6px 14px;
  border-bottom: 1px solid var(--line);
}

.brand-mark {
  width: 36px;
  height: 36px;
  display: grid;
  place-items: center;
  border-radius: 11px;
  font-weight: 800;
  color: #06121f;
  background: linear-gradient(135deg, var(--accent), var(--teal));
  box-shadow: 0 8px 24px rgba(95, 141, 255, 0.28);
}

.brand-text {
  display: flex;
  flex-direction: column;
  line-height: 1.15;
}

.brand-text strong {
  font-size: 15px;
  letter-spacing: 0.01em;
}

.brand-text span {
  font-size: 11px;
  color: var(--muted-2);
  letter-spacing: 0.22em;
  text-transform: uppercase;
}

.nav {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 10px 12px;
  border: 0;
  border-radius: 9px;
  background: transparent;
  color: var(--muted);
  cursor: pointer;
  text-align: left;
  transition: background 0.15s ease, color 0.15s ease;
}

.nav-item svg {
  width: 18px;
  height: 18px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.6;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.nav-item em {
  margin-left: auto;
  font-style: normal;
  font-size: 11px;
  color: var(--muted-2);
  background: var(--panel-2);
  border-radius: 99px;
  padding: 1px 7px;
}

.nav-item:hover {
  background: rgba(255, 255, 255, 0.04);
  color: var(--text);
}

.nav-item.active {
  background: linear-gradient(90deg, rgba(122, 162, 255, 0.18), rgba(122, 162, 255, 0.04));
  color: #dfe8ff;
  box-shadow: inset 2px 0 0 var(--accent);
}

.sidebar-foot {
  margin-top: auto;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 11px 12px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--panel);
  cursor: pointer;
  text-align: left;
}

.sidebar-foot:hover {
  background: var(--panel-2);
}

.status-dot {
  width: 8px;
  height: 8px;
  flex: none;
  border-radius: 50%;
  background: var(--danger);
  box-shadow: 0 0 0 4px rgba(255, 128, 128, 0.12);
}

.status-dot.ok {
  background: var(--ok);
  box-shadow: 0 0 0 4px rgba(90, 217, 164, 0.12);
}

.foot-text {
  display: flex;
  flex-direction: column;
  line-height: 1.25;
  min-width: 0;
}

.foot-text strong {
  font-size: 12px;
  font-weight: 600;
}

.foot-text span {
  font-size: 11px;
  color: var(--muted-2);
}

/* ---------------------------------------------------------------- workspace */

.workspace {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}

.topbar {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  padding: 22px 26px 18px;
  border-bottom: 1px solid var(--line);
}

.heading h1 {
  margin: 0;
  font-size: 22px;
  letter-spacing: -0.01em;
}

.heading p {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--muted);
}

.actions {
  display: flex;
  align-items: center;
  gap: 9px;
}

.search {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 34px;
  padding: 0 11px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel);
  color: var(--muted-2);
}

.search:focus-within {
  border-color: rgba(122, 162, 255, 0.55);
}

.search svg {
  width: 15px;
  height: 15px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.7;
  stroke-linecap: round;
}

.search input {
  width: 160px;
  border: 0;
  outline: 0;
  background: transparent;
  font-size: 13px;
}

.search input::placeholder {
  color: var(--muted-2);
}

.btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 7px;
  height: 34px;
  padding: 0 14px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel-2);
  color: var(--text);
  font-size: 13px;
  cursor: pointer;
  white-space: nowrap;
  transition: background 0.15s ease, border-color 0.15s ease, transform 0.1s ease;
}

.btn:hover:not(:disabled) {
  background: var(--panel-3);
  border-color: #2e394e;
}

.btn:active:not(:disabled) {
  transform: translateY(1px);
}

.btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

.btn.primary {
  border-color: transparent;
  color: #06121f;
  font-weight: 650;
  background: linear-gradient(120deg, #8ab4ff, var(--teal));
  box-shadow: 0 8px 26px rgba(95, 141, 255, 0.28);
}

.btn.primary:hover:not(:disabled) {
  filter: brightness(1.06);
}

.btn.block {
  width: 100%;
  height: 38px;
}

.btn.ghost {
  background: transparent;
}

.btn.danger {
  color: var(--danger);
  border-color: rgba(255, 128, 128, 0.24);
}

.btn.danger:hover:not(:disabled) {
  background: rgba(255, 128, 128, 0.08);
  border-color: rgba(255, 128, 128, 0.4);
}

.notice {
  flex: none;
  margin: 12px 26px 0;
  padding: 9px 13px;
  border-radius: 8px;
  font-size: 12.5px;
  border: 1px solid var(--line);
  background: var(--panel);
}

.notice.ok {
  color: #b7f4df;
  border-color: rgba(90, 217, 164, 0.3);
  background: rgba(90, 217, 164, 0.07);
}

.notice.error {
  color: #ffc2c2;
  border-color: rgba(255, 128, 128, 0.3);
  background: rgba(255, 128, 128, 0.07);
}

/* ---------------------------------------------------------------- project list */

.projects {
  flex: 1;
  min-height: 0;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 348px;
}

.list {
  min-height: 0;
  overflow-y: auto;
  padding: 14px 12px 20px 26px;
}

.project-row {
  display: flex;
  align-items: center;
  gap: 13px;
  padding: 13px 14px;
  margin-bottom: 6px;
  border: 1px solid transparent;
  border-radius: 10px;
  cursor: pointer;
  outline: none;
  transition: background 0.14s ease, border-color 0.14s ease;
}

.project-row:hover,
.project-row:focus-visible {
  background: rgba(255, 255, 255, 0.035);
  border-color: var(--line);
}

.project-row.selected {
  background: linear-gradient(90deg, rgba(122, 162, 255, 0.13), rgba(122, 162, 255, 0.03));
  border-color: rgba(122, 162, 255, 0.28);
}

.project-icon {
  width: 38px;
  height: 38px;
  flex: none;
  display: grid;
  place-items: center;
  border-radius: 9px;
  background: var(--panel-2);
  border: 1px solid var(--line);
  color: var(--accent);
}

.project-icon svg {
  width: 19px;
  height: 19px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.6;
  stroke-linejoin: round;
}

.project-icon.large {
  width: 46px;
  height: 46px;
  border-radius: 12px;
}

.project-main {
  min-width: 0;
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.project-title {
  display: flex;
  align-items: center;
  gap: 8px;
}

.project-title strong {
  font-size: 14px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-path {
  font-size: 11.5px;
  color: var(--muted-2);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.project-side {
  flex: none;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 4px;
  font-size: 11.5px;
  color: var(--muted-2);
}

.badge {
  flex: none;
  padding: 1px 7px;
  border-radius: 99px;
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.badge.running {
  color: #b7f4df;
  background: rgba(90, 217, 164, 0.14);
}

.badge.missing {
  color: #ffc2c2;
  background: rgba(255, 128, 128, 0.14);
}

.badge.warn {
  color: #ffd9a1;
  background: rgba(255, 193, 105, 0.14);
}

.empty-list {
  height: 100%;
  min-height: 320px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 9px;
  text-align: center;
  color: var(--muted);
}

.empty-list.compact {
  min-height: 200px;
}

.empty-list strong {
  color: var(--text);
  font-size: 15px;
}

.empty-list p {
  margin: 0;
  max-width: 360px;
  font-size: 12.5px;
  line-height: 1.6;
}

.empty-art {
  width: 64px;
  height: 64px;
  display: grid;
  place-items: center;
  border-radius: 18px;
  background: var(--panel);
  border: 1px solid var(--line);
  color: var(--muted-2);
}

.empty-art svg {
  width: 28px;
  height: 28px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.4;
  stroke-linejoin: round;
}

.empty-actions {
  display: flex;
  gap: 9px;
  margin-top: 8px;
}

.link-btn {
  border: 0;
  background: none;
  color: var(--accent);
  font-size: 12.5px;
  cursor: pointer;
}

/* ---------------------------------------------------------------- detail */

.detail {
  min-height: 0;
  overflow-y: auto;
  padding: 18px 26px 24px 20px;
  border-left: 1px solid var(--line);
  background: rgba(13, 17, 25, 0.55);
}

.detail-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--line);
}

.detail-title {
  min-width: 0;
}

.detail-title h2 {
  margin: 0 0 4px;
  font-size: 17px;
}

.detail-title p {
  margin: 0;
  font-size: 11px;
  color: var(--muted-2);
  word-break: break-all;
}

.detail-actions {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin: 16px 0;
}

.callout {
  padding: 9px 12px;
  border-radius: 8px;
  font-size: 12px;
  margin-bottom: 14px;
}

.callout.danger {
  color: #ffc2c2;
  background: rgba(255, 128, 128, 0.08);
  border: 1px solid rgba(255, 128, 128, 0.26);
}

.meta {
  margin: 0 0 18px;
  display: flex;
  flex-direction: column;
  gap: 1px;
  border: 1px solid var(--line);
  border-radius: 9px;
  overflow: hidden;
}

.meta > div {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 16px;
  padding: 9px 12px;
  background: var(--panel);
}

.meta > div + div {
  border-top: 1px solid var(--line);
}

.meta dt {
  flex: none;
  font-size: 11.5px;
  color: var(--muted-2);
}

.meta dd {
  margin: 0;
  font-size: 12px;
  text-align: right;
  word-break: break-all;
}

.detail-empty {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 6px;
  text-align: center;
  color: var(--muted);
}

.detail-empty strong {
  color: var(--text);
  font-size: 14px;
}

.detail-empty p {
  margin: 0;
  font-size: 12px;
}

/* ---------------------------------------------------------------- editor view */

.editor-view {
  flex: 1;
  overflow-y: auto;
  padding: 22px 26px;
}

.editor-card {
  max-width: 760px;
  padding: 22px;
  border: 1px solid var(--line);
  border-radius: 12px;
  background: var(--panel);
}

.editor-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-bottom: 18px;
  margin-bottom: 16px;
  border-bottom: 1px solid var(--line);
}

.editor-title {
  flex: 1;
  min-width: 0;
}

.editor-title h2 {
  margin: 0 0 4px;
  font-size: 16px;
}

.editor-title p {
  margin: 0;
  font-size: 12px;
  color: var(--muted);
}

.hint {
  margin: 14px 0 0;
  padding: 12px;
  border-radius: 8px;
  border: 1px solid var(--line);
  background: var(--panel-2);
  color: var(--muted);
  font-size: 12px;
  line-height: 1.65;
  white-space: pre-wrap;
  word-break: break-word;
}

.hint code {
  color: var(--teal);
  font-family: inherit;
}

/* ---------------------------------------------------------------- modal */

.modal-backdrop {
  position: fixed;
  inset: 0;
  z-index: 20;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgba(3, 5, 9, 0.66);
  backdrop-filter: blur(6px);
}

.modal {
  width: min(520px, 100%);
  padding: 22px;
  border: 1px solid var(--line);
  border-radius: 14px;
  background: var(--panel);
  box-shadow: 0 30px 80px rgba(0, 0, 0, 0.55);
}

.modal-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 18px;
}

.modal-head h2 {
  margin: 0 0 4px;
  font-size: 18px;
}

.modal-head p {
  margin: 0;
  font-size: 12px;
  color: var(--muted);
}

.icon-btn {
  width: 30px;
  height: 30px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--panel-2);
  color: var(--muted);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}

.icon-btn:hover {
  color: var(--text);
  background: var(--panel-3);
}

.field {
  display: flex;
  flex-direction: column;
  gap: 7px;
  margin-bottom: 14px;
}

.field > span {
  font-size: 12px;
  color: var(--muted);
}

.field input {
  height: 36px;
  padding: 0 11px;
  border: 1px solid var(--line);
  border-radius: 8px;
  outline: none;
  background: var(--panel-2);
  font-size: 13px;
}

.field input:focus {
  border-color: rgba(122, 162, 255, 0.6);
}

.field-row {
  display: flex;
  gap: 8px;
}

.field-row input {
  flex: 1;
  min-width: 0;
}

.path-preview {
  margin: -4px 0 14px;
  font-size: 11px;
  color: var(--muted-2);
  word-break: break-all;
}

.template {
  display: grid;
  gap: 8px;
  margin-bottom: 18px;
}

.template-card {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 12px 14px;
  border: 1px solid var(--line);
  border-radius: 9px;
  background: var(--panel-2);
}

.template-card.selected {
  border-color: rgba(122, 162, 255, 0.5);
  background: rgba(122, 162, 255, 0.08);
}

.template-card strong {
  font-size: 13px;
}

.template-card span {
  font-size: 11.5px;
  color: var(--muted);
}

.modal-foot {
  display: flex;
  justify-content: flex-end;
  gap: 9px;
}

/* ---------------------------------------------------------------- mobile */

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

@media (max-width: 900px) {
  .projects {
    grid-template-columns: minmax(0, 1fr);
  }

  .detail {
    display: none;
  }
}
</style>
