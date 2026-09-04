# Future Homebrew tap checklist

The Joocode Homebrew tap is not currently published. Users should install with
the verified release installer documented in the main README.

This document is only a maintainer checklist for enabling the tap later.

1. Keep GitHub releases public, immutable, and available without
   authentication. The formula downloads release archives directly.
2. Make sure the release workflow attaches all macOS and Linux archives plus
   `SHA256SUMS`.
3. Generate the formula from the release checksums:

   ```bash
   cargo xtask homebrew-formula vX.Y.Z SHA256SUMS
   ```

4. Test the formula locally:

   ```bash
   brew install --build-from-source ./joocode.rb
   brew test joocode
   brew audit --strict --online ./joocode.rb
   ```

5. Create the public `yan-ad/homebrew-tap` repository with a
   `Formula/joocode.rb` path.
6. Configure `HOMEBREW_TAP_TOKEN` in the Joocode repository with write access
   to that repository. Tagged releases can then update the formula
   automatically.
7. Only after the first formula is published, document these commands as active:

   ```bash
   brew tap yan-ad/tap
   brew install joocode
   ```
