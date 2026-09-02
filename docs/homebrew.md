# Homebrew packaging

The release workflow generates a checksum-pinned `joocode.rb` formula and
attaches it to every GitHub release.

> [!IMPORTANT]
> The `yan-ad/homebrew-tap` repository has not been published yet. Until it is
> available, the supported installation path is `install.bash` from the main
> README. Do not advertise `brew tap yan-ad/tap` as an active installation
> command.

## Install directly from a release

```bash
brew install https://github.com/yan-ad/joocode/releases/latest/download/joocode.rb
```

To install a specific release:

```bash
brew install https://github.com/yan-ad/joocode/releases/download/v0.1.1/joocode.rb
```

## Install from a tap

After maintainers create and configure `yan-ad/homebrew-tap`, users will be able
to install with:

```bash
brew tap yan-ad/tap
brew install yan-ad/tap/joocode
jcx --version
```

The release workflow can publish to that tap when the repository exists and has
a
`HOMEBREW_TAP_TOKEN` secret with permission to write to the tap repository.
Set the `HOMEBREW_TAP_REPOSITORY` repository variable if the tap is hosted
elsewhere; its default is `yan-ad/homebrew-tap`.
