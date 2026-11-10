# macOS bundle
# script/package-rust.sh macos → dist/GitHub Desktop-macos-arm64/ (or x64)
# The .app bundle is created by `platforms` packaging from this staging dir.
# Signing/notarisation uses the fork's own identity; upstream distribution
# identities are not reused. Bundle identifier is the fork's (com.github.GitHubClient
# is replaced by the fork identity once the OAuth callback is registered — see
# docs/migration/status.md §9.1 §8).
