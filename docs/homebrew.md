# Homebrew packaging

The release workflow generates a checksum-pinned `joocode.rb` formula and
attaches it to every GitHub release.

## Install directly from a release

```bash
brew install https://github.com/yan-ad/joc/releases/latest/download/joocode.rb
```

To install a specific release:

```bash
brew install https://github.com/yan-ad/joc/releases/download/v0.1.1/joocode.rb
```

## Install from a tap

For a stable tap experience, maintainers can publish the generated formula to
`yan-ad/homebrew-tap`:

```bash
brew tap yan-ad/tap
brew install yan-ad/tap/joocode
```

The release workflow publishes to that tap when the repository has a
`HOMEBREW_TAP_TOKEN` secret with permission to write to the tap repository.
Set the `HOMEBREW_TAP_REPOSITORY` repository variable if the tap is hosted
elsewhere; its default is `yan-ad/homebrew-tap`.
