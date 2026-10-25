#!/usr/bin/env python3
"""校验 commit 标题是否符合 docs/ai-workflow.md §3。

由 .githooks/commit-msg 调用；也可以手动跑：

    python scripts/check_commit_msg.py .git/COMMIT_EDITMSG
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

TYPES = ("feat", "fix", "refactor", "perf", "docs", "test", "chore", "ci", "build")
PATTERN = re.compile(
    rf"^({'|'.join(TYPES)})(\([a-z0-9][a-z0-9._/-]*\))?!?: \S"
)
ALLOWED_PREFIXES = ("Merge ", 'Revert "', "fixup! ", "squash! ", "amend! ")
MAX_SUBJECT = 72


def main() -> int:
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    if len(sys.argv) != 2:
        print("usage: check_commit_msg.py <commit-msg-file>", file=sys.stderr)
        return 2

    path = Path(sys.argv[1])
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    subject = next(
        (line.strip() for line in lines if line.strip() and not line.lstrip().startswith("#")),
        "",
    )

    if not subject:
        print("commit-msg: 标题为空", file=sys.stderr)
        return 1
    if subject.startswith(ALLOWED_PREFIXES):
        return 0
    if not PATTERN.match(subject):
        print(
            "commit-msg: 标题不符合规范\n"
            "  期望: <type>(<scope>): <祈使句摘要>\n"
            f"  type: {' | '.join(TYPES)}\n"
            f"  实际: {subject}\n"
            "  详见 docs/ai-workflow.md §3",
            file=sys.stderr,
        )
        return 1
    if len(subject) > MAX_SUBJECT:
        print(
            f"commit-msg: 标题 {len(subject)} 字符，超过 {MAX_SUBJECT}；"
            "把细节移到正文。",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
