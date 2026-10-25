#!/usr/bin/env python3
"""PrismaRev 提交前自检：一条命令，一个可粘贴到 PR 的 Markdown 块。

    python scripts/pr_ready.py                 # 完整自检（默认）
    python scripts/pr_ready.py --quick         # 快速回路：只做 fmt + clippy
    python scripts/pr_ready.py --all           # 全 workspace + 所有独立 workspace
    python scripts/pr_ready.py --base origin/master
    python scripts/pr_ready.py --verbose       # 失败时打印完整输出

它根据分支相对 --base 的改动（含未提交、未跟踪文件）决定要跑什么：

  * 只对**改动过的 .rs 文件**跑 rustfmt（仓库既有 fmt 漂移不算在每个 PR 头上）；
  * 改动到的根 workspace crate：clippy --all-targets -D warnings + cargo test；
  * 改到 shader/reflection/bindings：在临时目录重跑 shader-bindgen，比对是否漂移；
  * 改到 editor/：pnpm build；schema 变更时比对 fixture 的 schema/templates 是否过期；
  * 改到 projects/*、launcher/、editor/src-tauri/：各自 cargo check（独立 workspace）；
  * 拦截生成物 / 签名材料（.spv、.pak、keystore 等）和空白错误。

stdout 只输出 Markdown；进度和失败详情走 stderr。任何检查失败 → 退出码 1。
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IS_WINDOWS = os.name == "nt"

# 这些文件变化会影响整个根 workspace。
WORKSPACE_WIDE = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
}
WORKSPACE_PREFIXES = (".cargo/",)

# CI 的 desktop job 不覆盖 prism-app（它只作为用户项目依赖被构建）。
# 本地默认也跳过，除非它自己改动或 --all；projects/prisma 的 check 会覆盖它。
SKIP_BY_DEFAULT = {"prism-app"}

BAD_SUFFIXES = (
    ".spv",
    ".pak",
    ".pak.meta.json",
    ".keystore",
    ".jks",
    ".p12",
    ".pfx",
    ".pem",
    ".key",
)
BAD_PARTS = ("node_modules/", "target/", "/target/", "dist/")
BAD_NAMES = {".env", "local.properties", "id_rsa", "id_ed25519"}


# --------------------------------------------------------------------------- #
# 基础设施
# --------------------------------------------------------------------------- #
def eprint(*args: object) -> None:
    print(*args, file=sys.stderr, flush=True)


def run(cmd: list[str], cwd: Path = ROOT) -> tuple[int, str]:
    if cmd:
        # Windows 上 npm/pnpm/tsc 是 .cmd shim，CreateProcess 不会自动补 PATHEXT。
        resolved = shutil.which(cmd[0])
        if resolved:
            cmd = [resolved, *cmd[1:]]
    try:
        proc = subprocess.run(
            cmd,
            cwd=str(cwd),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except FileNotFoundError:
        return 127, f"command not found: {cmd[0]}"
    return proc.returncode, proc.stdout or ""


def git(*args: str) -> tuple[int, str]:
    return run(["git", *args])


def ensure_toolchain_in_path() -> None:
    """从 Explorer / 新 shell 启动 python 时，cargo 可能不在 PATH。"""
    cargo = shutil.which("cargo")
    if cargo:
        return
    candidates = [Path.home() / ".cargo" / "bin"]
    if IS_WINDOWS:
        candidates.append(Path(os.environ.get("USERPROFILE", "")) / ".cargo" / "bin")
    for candidate in candidates:
        exe = candidate / ("cargo.exe" if IS_WINDOWS else "cargo")
        if exe.exists():
            os.environ["PATH"] = str(candidate) + os.pathsep + os.environ.get("PATH", "")
            return


def resolve_base(explicit: str | None) -> str:
    if explicit:
        return explicit
    for ref in ("origin/master", "origin/main", "master", "main"):
        if git("rev-parse", "--verify", "--quiet", ref)[0] == 0:
            return ref
    return "HEAD"


def changed_files(base: str) -> list[str]:
    files: set[str] = set()
    probes = (
        ("diff", "--name-only", f"{base}...HEAD"),
        ("diff", "--name-only", "--cached"),
        ("diff", "--name-only"),
        ("ls-files", "--others", "--exclude-standard"),
    )
    for probe in probes:
        code, out = git(*probe)
        if code == 0:
            files.update(line.strip() for line in out.splitlines() if line.strip())
    return sorted(f.replace("\\", "/") for f in files if not f.startswith(".zcode/"))


def workspace_members() -> list[str]:
    try:
        import tomllib

        data = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        return [Path(member).name for member in data["workspace"]["members"]]
    except Exception:
        text = (ROOT / "Cargo.toml").read_text(encoding="utf-8", errors="replace")
        return re.findall(r'"crates/([^"/]+)"', text)


def has_prefix(paths: list[str], prefix: str) -> bool:
    return any(path.startswith(prefix) for path in paths)


def changed_rs(paths: list[str]) -> list[str]:
    return [p for p in paths if p.endswith(".rs") and (ROOT / p).is_file()]


def display_cmd(cmd: list[str], cwd: Path) -> str:
    rel = cwd.relative_to(ROOT) if cwd != ROOT else Path(".")
    prefix = "" if rel == Path(".") else f"(cd {rel.as_posix()} && "
    suffix = "" if rel == Path(".") else ")"
    return prefix + " ".join(cmd) + suffix


# --------------------------------------------------------------------------- #
# 检查项
# --------------------------------------------------------------------------- #
class Check:
    def __init__(self, name: str, cmd: list[str], cwd: Path = ROOT, detail: str = "") -> None:
        self.name = name
        self.cmd = cmd
        self.cwd = cwd
        self.detail = detail
        self.status = "skip"
        self.output = ""
        self.done = False

    def run(self, verbose: bool) -> bool:
        self.done = True
        eprint(f"→ {self.name}: {display_cmd(self.cmd, self.cwd)}")
        code, out = run(self.cmd, self.cwd)
        self.output = out
        if code == 0:
            self.status = "ok"
            eprint("  ok")
            return True
        self.status = "fail"
        eprint(f"  FAILED (exit {code})")
        if verbose:
            eprint(out)
        else:
            tail = "\n".join(out.rstrip().splitlines()[-25:])
            eprint(tail)
        return False


def test_summary(output: str) -> str:
    passed = sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed", output))
    failed = sum(int(n) for n in re.findall(r"test result: FAILED\. (\d+) passed", output))
    if failed:
        return f"{passed} passed, {failed} failed"
    return f"{passed} passed" if passed else "ok"


def shader_bindings_check(changed: list[str]) -> Check:
    """在临时目录重跑 bindgen，与已提交的 bindings 比对；不修改工作区。"""
    check = Check("shader bindings drift", ["shader-bindgen", "(temp out)"])
    check.done = True
    touched = has_prefix(changed, "assets/shaders/") or has_prefix(
        changed, "crates/prism-render/src/shader_bindings/"
    )
    if not touched:
        check.status = "skip"
        check.detail = "no shader changes"
        return check

    tmp = Path(tempfile.mkdtemp(prefix="prism-bindgen-"))
    try:
        eprint("→ shader bindings drift: regenerate into temp dir")
        code, out = run(
            [
                "cargo",
                "run",
                "--quiet",
                "--manifest-path",
                str(ROOT / "crates" / "xtask" / "Cargo.toml"),
                "--bin",
                "shader-bindgen",
                "--",
                str(ROOT / "assets" / "shaders" / "reflection"),
                str(tmp),
            ]
        )
        if code != 0:
            check.status = "fail"
            check.output = out
            check.detail = "bindgen failed"
            eprint("  FAILED (bindgen)")
            eprint("\n".join(out.rstrip().splitlines()[-25:]))
            return check

        committed = ROOT / "crates" / "prism-render" / "src" / "shader_bindings"
        diffs: list[str] = []
        generated = sorted(p for p in tmp.rglob("*") if p.is_file())
        for path in generated:
            rel = path.relative_to(tmp)
            current = committed / rel
            if not current.is_file() or current.read_bytes() != path.read_bytes():
                diffs.append(rel.as_posix())
        committed_names = {
            p.relative_to(committed).as_posix()
            for p in committed.rglob("*")
            if p.is_file()
        }
        generated_names = {p.relative_to(tmp).as_posix() for p in generated}
        diffs.extend(sorted(committed_names - generated_names))

        if diffs:
            check.status = "fail"
            check.detail = f"{len(diffs)} file(s) stale: {', '.join(diffs[:4])}"
            eprint(f"  FAILED: {check.detail}")
            eprint("  Run: bash assets/shaders/compile.sh && xtask shader-bindgen, then commit.")
        else:
            check.status = "ok"
            check.detail = "up to date"
            eprint("  ok")
        return check
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def artifact_guard(changed: list[str]) -> Check:
    check = Check("artifact / secret guard", ["git", "diff --name-only"])
    check.done = True
    eprint("→ artifact / secret guard")
    bad: list[str] = []
    for path in changed:
        name = Path(path).name
        lowered = path.lower()
        if lowered.endswith(BAD_SUFFIXES) or name in BAD_NAMES:
            bad.append(path)
        elif any(part in path for part in BAD_PARTS):
            bad.append(path)
    if bad:
        check.status = "fail"
        check.detail = f"must not be committed: {', '.join(bad[:5])}"
        eprint(f"  FAILED: {check.detail}")
        return check
    check.status = "ok"
    check.detail = f"{len(changed)} file(s) checked"
    eprint("  ok")
    return check


def whitespace_check() -> Check:
    check = Check("whitespace / conflict markers", ["git", "diff", "--check"])
    check.done = True
    eprint("→ whitespace / conflict markers")
    for args in (("diff", "--check"), ("diff", "--cached", "--check")):
        code, out = git(*args)
        if code != 0:
            check.status = "fail"
            check.output = out
            check.detail = out.strip().splitlines()[0] if out.strip() else "diff --check failed"
            eprint(f"  FAILED: {check.detail}")
            return check
    check.status = "ok"
    check.detail = "clean"
    eprint("  ok")
    return check


def format_check(files: list[str], verbose: bool) -> Check:
    check = Check("format (changed .rs)", ["rustfmt", "--check"])
    check.done = True
    eprint("→ format (changed .rs)")
    if not files:
        check.status = "skip"
        check.detail = "no .rs changes"
        return check
    rustfmt = shutil.which("rustfmt")
    if not rustfmt:
        check.status = "skip"
        check.detail = "rustfmt not on PATH"
        eprint("  skipped: rustfmt not on PATH")
        return check

    failures: list[str] = []
    for path in files:
        cwd = ROOT
        code, out = run([rustfmt, "--edition", "2021", "--check", str(ROOT / path)], cwd)
        if code != 0:
            failures.append(path)
            if verbose:
                eprint(out)
    if failures:
        check.status = "fail"
        check.detail = f"{len(failures)} file(s): {', '.join(failures[:4])}"
        eprint(f"  FAILED: {check.detail}")
        eprint("  Run: cargo fmt --all  (or rustfmt on the listed files)")
    else:
        check.status = "ok"
        check.detail = f"{len(files)} file(s)"
        eprint("  ok")
    return check


def editor_fixture_check(changed: list[str]) -> Check:
    check = Check(
        "editor fixture drift",
        ["cargo", "run", "-q", "-p", "prism-editor-api", "--example", "dump_fixture"],
    )
    check.done = True
    relevant = (
        "crates/prism-scene/src/components.rs",
        "crates/prism-scene/src/ui.rs",
        "editor/public/demo.json",
    )
    if not (has_prefix(changed, "crates/prism-editor-api/") or any(p in changed for p in relevant)):
        check.status = "skip"
        check.detail = "schema untouched"
        return check

    eprint("→ editor fixture drift: regenerate and compare schema/templates")
    code, out = run(
        ["cargo", "run", "-q", "-p", "prism-editor-api", "--example", "dump_fixture"]
    )
    if code != 0:
        check.status = "fail"
        check.detail = "dump_fixture failed"
        check.output = out
        eprint("\n".join(out.rstrip().splitlines()[-25:]))
        return check

    import json

    # 只比注册表派生的部分；assets/docs 依赖本地工作区（未提交场景、external 素材）。
    keys = ("schema", "presets", "templatesByKind", "templateDocs")
    try:
        fresh = json.loads(out)
        committed = json.loads((ROOT / "editor" / "public" / "demo.json").read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        check.status = "fail"
        check.detail = f"invalid fixture JSON: {exc}"
        return check

    stale = [key for key in keys if fresh.get(key) != committed.get(key)]
    if stale:
        check.status = "fail"
        check.detail = f"stale: {', '.join(stale)} (run `cd editor && pnpm fixture`)"
        eprint(f"  FAILED: {check.detail}")
    else:
        check.status = "ok"
        check.detail = "schema/templates up to date"
        eprint("  ok")
    return check


def editor_build_check(changed: list[str]) -> Check:
    check = Check("editor build", ["pnpm", "build"], ROOT / "editor")
    if not has_prefix(changed, "editor/"):
        check.status = "skip"
        check.detail = "editor untouched"
        check.done = True
        return check
    if not (ROOT / "editor" / "node_modules").is_dir():
        check.status = "skip"
        check.detail = "node_modules missing: cd editor && pnpm install"
        check.done = True
        return check
    if not shutil.which("pnpm"):
        check.status = "skip"
        check.detail = "pnpm not on PATH"
        check.done = True
        return check
    return check


# --------------------------------------------------------------------------- #
# 主流程
# --------------------------------------------------------------------------- #
def main() -> int:
    parser = argparse.ArgumentParser(description="PrismaRev pre-PR self-check")
    parser.add_argument("--base", help="比较基准（默认 origin/master）")
    parser.add_argument("--quick", action="store_true", help="只做 fmt + clippy，不跑测试")
    parser.add_argument("--all", action="store_true", help="检查所有 workspace 与独立项目")
    parser.add_argument("--verbose", action="store_true", help="失败时打印完整输出")
    args = parser.parse_args()

    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    ensure_toolchain_in_path()
    base = resolve_base(args.base)
    changed = changed_files(base)
    rs_files = changed_rs(changed)

    members = workspace_members()
    crates: set[str] = set()
    if args.all:
        crates = set(members)
    else:
        for path in changed:
            if path.startswith("crates/"):
                parts = path.split("/")
                if len(parts) > 1 and parts[1] in members:
                    crates.add(parts[1])
        if any(p in WORKSPACE_WIDE or p.startswith(WORKSPACE_PREFIXES) for p in changed):
            crates = set(members)

    default_crates = sorted(crates if args.all else crates - SKIP_BY_DEFAULT)
    eprint(f"PrismaRev self-check — base {base}, {len(changed)} changed file(s)")
    eprint(f"  touched crates: {', '.join(default_crates) or '(none)'}")

    checks: list[Check] = [whitespace_check(), artifact_guard(changed)]
    checks.append(format_check(rs_files, args.verbose))

    if default_crates:
        clippy_cmd = ["cargo", "clippy"]
        for crate in default_crates:
            clippy_cmd += ["-p", crate]
        clippy_cmd += ["--all-targets", "--", "-D", "warnings"]
        checks.append(Check(f"clippy ({len(default_crates)} crate{'s' if len(default_crates) > 1 else ''})", clippy_cmd))

        if not args.quick:
            test_cmd = ["cargo", "test"]
            for crate in default_crates:
                test_cmd += ["-p", crate]
            test_cmd += ["--lib", "--bins", "--tests"]
            checks.append(Check(f"test ({len(default_crates)} crate{'s' if len(default_crates) > 1 else ''})", test_cmd))

    if args.all or has_prefix(changed, "crates/xtask/"):
        checks.append(
            Check(
                "xtask check",
                ["cargo", "check", "--manifest-path", str(ROOT / "crates" / "xtask" / "Cargo.toml")],
            )
        )

    checks.append(shader_bindings_check(changed))

    if not args.quick:
        checks.append(editor_fixture_check(changed))
        checks.append(editor_build_check(changed))

        standalone = [
            ("projects/prisma", "projects/prisma/"),
            ("projects/prisma-sponza", "projects/prisma-sponza/"),
            ("launcher", "launcher/"),
            ("editor/src-tauri", "editor/src-tauri/"),
        ]
        for name, prefix in standalone:
            manifest = ROOT / name / "Cargo.toml"
            if manifest.is_file() and (args.all or has_prefix(changed, prefix)):
                checks.append(
                    Check(
                        f"{name} check",
                        ["cargo", "check", "--manifest-path", str(manifest)],
                    )
                )

    failed = False
    for check in checks:
        if check.done:
            continue
        if not check.run(args.verbose):
            failed = True
    if not failed:
        failed = any(check.status == "fail" for check in checks)

    # 特定检查的 detail 需要在运行后才能确定。
    for check in checks:
        if check.name.startswith("test (") and check.status == "ok":
            check.detail = test_summary(check.output)
        elif check.name.startswith("clippy (") and check.status == "ok":
            check.detail = "-D warnings"

    sha_code, sha_out = git("rev-parse", "--short", "HEAD")
    sha = sha_out.strip() if sha_code == 0 else "unknown"
    dirty = bool(git("status", "--porcelain")[1].strip())
    mode = "quick" if args.quick else ("all" if args.all else "full")

    lines = [
        "## Self-check (`scripts/pr_ready.py`)",
        "",
        f"- mode: **{mode}** · base: `{base}` · HEAD: `{sha}`"
        + (" · working tree dirty" if dirty else "")
        + f" · changed: {len(changed)} file(s)",
        f"- crates: {', '.join(f'`{c}`' for c in default_crates) or '—'}",
        "",
        "| Check | Result | Detail |",
        "|---|---|---|",
    ]
    icons = {"ok": "✅", "fail": "❌", "skip": "⏭️"}
    for check in checks:
        detail = check.detail.replace("|", "\\|")
        lines.append(f"| {check.name} | {icons[check.status]} | {detail} |")
    lines.append("")
    lines.append("**Result: ✅ all checks passed**" if not failed else "**Result: ❌ checks failed**")

    # 未运行的 skip 项没有经过 run()，这里也把它们标出来。
    print("\n".join(lines))
    sys.stdout.flush()

    if failed and not shutil.which("slangc"):
        eprint("")
        eprint("提示：本机没有 slangc。编译依赖 assets/shaders/*.spv；")
        eprint("     请先运行 scripts/run.ps1，或 bash assets/shaders/compile.sh。")

    eprint("")
    eprint("PASS" if not failed else "FAIL")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
