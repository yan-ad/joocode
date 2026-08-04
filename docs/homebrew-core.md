# Homebrew Core Submission

Once Joocode has a stable release history, submit it to
[`Homebrew/homebrew-core`](https://github.com/Homebrew/homebrew-core) so users
can install it with:

```bash
brew install joocode
```

Until Homebrew merges the formula, the supported commands are:

```bash
brew install https://github.com/yan-ad/joc/releases/latest/download/joocode.rb
# or
brew install yan-ad/tap/joocode
```

## Maintainer checklist

1. Keep GitHub releases public, immutable, and available without
   authentication. The formula downloads release archives directly.
2. Make sure the release workflow attaches all macOS and Linux archives plus
   `SHA256SUMS`.
3. Generate the formula from the release checksums:

   ```bash
   python3 scripts/generate-homebrew-formula.py vX.Y.Z SHA256SUMS
   ```

4. Test the formula locally:

   ```bash
   brew install --build-from-source ./joocode.rb
   brew test joocode
   brew audit --strict --online ./joocode.rb
   ```

5. Fork `Homebrew/homebrew-core`, add the formula as
   `Formula/j/joocode.rb`, and open a pull request that follows its
   [acceptable formulae policy](https://docs.brew.sh/Acceptable-Formulae).

## Formula requirements

Homebrew core review may request changes to the description, test block,
download layout, or release history. Do not claim `brew install joocode` is
available until that pull request is merged. After approval, the existing tap
and direct-release formula may remain available as alternatives.
