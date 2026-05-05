# Contributing to Slate Framework

## Releases

### Versioning

This project follows [Semantic Versioning](https://semver.org/):
- **MAJOR**: Breaking API changes
- **MINOR**: New features, backwards compatible
- **PATCH**: Bug fixes, backwards compatible

### Creating a Release

1. Update `CHANGELOG.md`: move items from `[Unreleased]` to a new version section with release date, and update the comparison links at the bottom.

2. Update the version in `Cargo.toml`:
   ```toml
   [workspace.package]
   version = "X.Y.Z"
   ```

3. Commit the changes:
   ```bash
   git add CHANGELOG.md Cargo.toml
   git commit -m "release: prepare vX.Y.Z"
   ```

4. Create and push the tag:
   ```bash
   git tag vX.Y.Z
   git push origin main --tags
   ```

5. GitHub Actions will automatically:
   - Run CI checks (check, clippy, fmt, deny, audit, build, test)
   - Build release artifacts for Windows and macOS
   - Create a GitHub release with changelog notes
   - Mark pre-releases correctly for `alpha`, `beta`, or `rc` tags
