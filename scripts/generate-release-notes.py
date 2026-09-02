#!/usr/bin/env python3
"""Generate deterministic GitHub release notes from conventional commits."""

from __future__ import annotations

from collections import OrderedDict
from pathlib import Path
import os
import re
import subprocess
import sys


def git(*args: str) -> str:
    return subprocess.check_output(["git", *args], text=True).strip()


def previous_tag(tag: str) -> str | None:
    tags = git("tag", "--merged", f"{tag}^{{}}", "--sort=-version:refname").splitlines()
    return next((candidate for candidate in tags if candidate != tag), None)


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit(f"usage: {sys.argv[0]} TAG OUTPUT")

    tag, output = sys.argv[1:]
    git("rev-parse", "--verify", f"refs/tags/{tag}")
    previous = previous_tag(tag)
    revision_range = f"{previous}..{tag}" if previous else tag
    repository = os.environ.get("GITHUB_REPOSITORY", "yan-ad/joocode")

    categories: OrderedDict[str, list[str]] = OrderedDict(
        [
            ("Features", []),
            ("Fixes", []),
            ("Performance", []),
            ("Documentation", []),
            ("Build and maintenance", []),
            ("Other changes", []),
        ]
    )
    headings = {
        "feat": "Features",
        "fix": "Fixes",
        "perf": "Performance",
        "docs": "Documentation",
        "build": "Build and maintenance",
        "ci": "Build and maintenance",
        "chore": "Build and maintenance",
        "refactor": "Build and maintenance",
        "test": "Build and maintenance",
    }
    conventional = re.compile(
        r"^(?P<kind>[a-z]+)(?:\([^)]+\))?(?P<breaking>!)?:\s*(?P<title>.+)$"
    )

    log = git("log", "--format=%H%x09%s", revision_range)
    for line in log.splitlines():
        commit, subject = line.split("\t", 1)
        if re.match(r"^chore(?:\([^)]+\))?: release v", subject, re.IGNORECASE):
            continue
        match = conventional.match(subject)
        if match:
            title = match.group("title")
            if match.group("breaking"):
                title = f"**Breaking:** {title}"
            heading = headings.get(match.group("kind"), "Other changes")
        else:
            title = subject
            heading = "Other changes"
        short = commit[:7]
        categories[heading].append(
            f"- {title} ([`{short}`](https://github.com/{repository}/commit/{commit}))"
        )

    lines = [f"# Joocode {tag}", "", "## What's changed", ""]
    if not any(categories.values()):
        lines.extend(["- No user-facing changes were recorded.", ""])
    else:
        for heading, entries in categories.items():
            if entries:
                lines.extend([f"### {heading}", "", *entries, ""])

    if previous:
        lines.extend(
            [
                "## Full changelog",
                "",
                f"[{previous}...{tag}](https://github.com/{repository}/compare/{previous}...{tag})",
                "",
            ]
        )

    Path(output).write_text("\n".join(lines), encoding="utf-8")


if __name__ == "__main__":
    main()
