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

repo = os.environ.get("GITHUB_REPOSITORY", "yan-ad/joocode")
assets = {
    "arm64_macos": f"joocode-aarch64-apple-darwin.tar.gz",
    "x86_macos": f"joocode-x86_64-apple-darwin.tar.gz",
    "arm64_linux": f"joocode-aarch64-unknown-linux-gnu.tar.gz",
    "x86_linux": f"joocode-x86_64-unknown-linux-gnu.tar.gz",
}
missing = [name for name in assets.values() if name not in values]
if missing:
    raise SystemExit(f"missing checksums: {', '.join(missing)}")

base = f"https://github.com/{repo}/releases/download/v{version}"

def url(asset: str) -> str:
    return f"{base}/{asset}"

formula = f'''class Joocode < Formula
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
    bin.install Dir["joocode-*/jcx"].first => "jcx"
    bin.install Dir["joocode-*/joocode"].first => "joocode"
  end

  test do
    assert_match "jcx", shell_output("#{{bin}}/jcx --help")
  end
end
'''

Path("joocode.rb").write_text(formula)
print(formula, end="")
