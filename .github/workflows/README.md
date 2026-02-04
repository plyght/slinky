# GitHub Actions Workflows

## Release Build (`release.yml`)

Automated release pipeline triggered on version bumps in `Cargo.toml`.

### Trigger
- Push to `main` or `master` branch
- Changes to `Cargo.toml` file

### Required Secrets

#### `OPENAI_API_KEY`
OpenAI API key for generating AI-powered release notes.
- **How to add**: Repository Settings → Secrets and variables → Actions → New repository secret
- **Model used**: `gpt-5.2` (configurable in workflow)

#### `CARGO_TOKEN`
Crates.io API token for publishing packages.
- **How to get**: Visit https://crates.io/settings/tokens
- **How to add**: Repository Settings → Secrets and variables → Actions → New repository secret

### Workflow Steps

1. **Version Check**: Compares current `Cargo.toml` version with previous commit
   - Skips if version unchanged
   - Skips if tag already exists

2. **AI Release Notes**: Uses OpenAI API to generate release notes from git diff
   - Analyzes `.rs` and `Cargo.toml` changes
   - Fallback to changelog link if API fails

3. **Tag Creation**: Creates and pushes `v{version}` tag

4. **GitHub Release**: Creates release with AI-generated notes

5. **Multi-Platform Build**: Compiles binaries for:
   - Linux x64
   - Linux ARM64
   - macOS x64 (Intel)
   - macOS ARM64 (Apple Silicon)

6. **Crates.io Publish**: Publishes package to crates.io (continues on error)

### Usage

To trigger a release:
1. Bump version in `Cargo.toml`
2. Commit and push to main/master
3. Workflow runs automatically

### Output Artifacts

Each release includes:
- AI-generated release notes
- Binary downloads for all platforms:
  - `slnky-linux-x64`
  - `slnky-linux-arm64`
  - `slnky-macos-x64`
  - `slnky-macos-arm64`
- Package published to crates.io

### Customization

To modify the AI prompt or model:
- Edit the `generate-notes` job
- Change `model` field (default: `gpt-5.2`)
- Adjust system prompt for different note styles
