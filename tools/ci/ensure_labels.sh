#!/bin/sh
# 创建 Issue / PR 自动化使用的标签。可重复运行；已存在的标签保持原样。
#
#   sh tools/ci/ensure_labels.sh            # 当前仓库
#   sh tools/ci/ensure_labels.sh owner/repo # 指定仓库
set -eu

repo=${1:-${GITHUB_REPOSITORY:-}}
repo_flag=""
[ -n "$repo" ] && repo_flag="-R $repo"

create() {
    name=$1
    color=$2
    description=$3
    # shellcheck disable=SC2086 # repo_flag 需要按词拆分成 -R <repo>
    gh label create "$name" --color "$color" --description "$description" $repo_flag \
        >/dev/null 2>&1 || true
}

# 区域
create "area/render"    "1f77b4" "Vulkan 渲染、shader、swapchain"
create "area/scene"     "2ca02c" "prism-scene / 反射 / RSCN"
create "area/editor"    "9467bd" "Web 编辑器与 prism-editor-api"
create "area/engine"    "ff7f0e" "engine / app / platform / audio"
create "area/assets"    "8c564b" "asset / cooker / build-pipeline"
create "area/game"      "e377c2" "projects/ 用户游戏项目"
create "area/android"   "00a4a6" "Android 打包、launcher、JNI"
create "area/ci"        "7f7f7f" "CI、脚本、xtask"
create "area/docs"      "bcbd22" "文档"

# 规模
create "size/s"         "0e8a16" "改动 < 100 行"
create "size/m"         "fbca04" "改动 100–499 行"
create "size/l"         "d93f0b" "改动 >= 500 行（应说明或拆分）"

# 流程
create "ai"             "5319e7" "AI agent 参与"
create "needs/title"    "b60205" "PR 标题不符合 Conventional Commits"
create "needs/self-check" "b60205" "PR 缺少 pr_ready 自检块"
create "needs/triage"   "d4c5f9" "等待人工 / agent 分诊"
create "source/discord" "5865f2" "由 Discord bot 转入的 issue"

# Agent 认领
create "agent/ready"    "0e8a16" "可被 agent 认领"
create "agent/working"  "1d76db" "已有 agent 在做"
create "agent/blocked"  "b60205" "agent 被阻塞，需要人"
