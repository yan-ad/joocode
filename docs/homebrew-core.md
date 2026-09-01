# Homebrew Packaging

Joocode is distributed through its official Homebrew tap. Users should install
it with:

```bash
brew tap yan-ad/tap
brew install joocode
jcx --version
```

Or use the fully qualified one-command form:

```bash
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

5. Configure `HOMEBREW_TAP_TOKEN` in the Joocode repository with write access
   to `yan-ad/homebrew-tap`. Each tagged release will then update
   `Formula/joocode.rb` in the tap automatically.
