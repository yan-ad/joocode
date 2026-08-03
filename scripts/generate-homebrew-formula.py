#!/usr/bin/env python3
"""Generate a Homebrew formula for a JustOpenCode release."""

from pathlib import Path
import os
import sys

if len(sys.argv) != 3:
    raise SystemExit(f"usage: {sys.argv[0]} VERSION SHA256SUMS")

version = sys.argv[1].removeprefix("v")
checksums = Path(sys.argv[2]).read_text().splitlines()
values = {}
for line in checksums:
    digest, filename = line.split(maxsplit=1)
    values[filename.lstrip("*")] = digest

repo = os.environ.get("GITHUB_REPOSITORY", "yan-ad/joc")
assets = {
    "arm64_macos": f"joc-aarch64-apple-darwin.tar.gz",
    "x86_macos": f"joc-x86_64-apple-darwin.tar.gz",
    "arm64_linux": f"joc-aarch64-unknown-linux-gnu.tar.gz",
    "x86_linux": f"joc-x86_64-unknown-linux-gnu.tar.gz",
}
missing = [name for name in assets.values() if name not in values]
if missing:
    raise SystemExit(f"missing checksums: {', '.join(missing)}")

base = f"https://github.com/{repo}/releases/download/v{version}"

def url(asset: str) -> str:
    return f"{base}/{asset}"

formula = f'''class Joc < Formula
  desc "Native bridge from OpenCode providers to the OpenAI Responses API"
  homepage "https://github.com/{repo}"
  version "{version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "{url(assets["arm64_macos"])}"
      sha256 "{values[assets["arm64_macos"]]}"
    else
      url "{url(assets["x86_macos"])}"
      sha256 "{values[assets["x86_macos"]]}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "{url(assets["arm64_linux"])}"
      sha256 "{values[assets["arm64_linux"]]}"
    else
      url "{url(assets["x86_linux"])}"
      sha256 "{values[assets["x86_linux"]]}"
    end
  end

  def install
    bin.install Dir["joc-*/joc"].first => "joc"
  end

  test do
    assert_match "joc", shell_output("#{{bin}}/joc --help")
  end
end
'''

Path("joc.rb").write_text(formula)
print(formula, end="")
