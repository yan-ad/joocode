# Homebrew packaging

The release workflow generates a checksum-pinned `crabcodex.rb` formula and
attaches it to every GitHub release.

## Install directly from a release

```bash
brew install https://github.com/yan-ad/crabcodex/releases/latest/download/crabcodex.rb
```

To install a specific release:

```bash
brew install https://github.com/yan-ad/crabcodex/releases/download/v0.1.0/crabcodex.rb
```

## Install from a tap

For a stable tap experience, maintainers can publish the generated formula to
`yan-ad/homebrew-tap`:

```bash
brew tap yan-ad/tap
brew install yan-ad/tap/crabcodex
```

The release workflow publishes to that tap when the repository has a
`HOMEBREW_TAP_TOKEN` secret with permission to write to the tap repository.
Set the `HOMEBREW_TAP_REPOSITORY` repository variable if the tap is hosted
elsewhere; its default is `yan-ad/homebrew-tap`.
