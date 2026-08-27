# Changelog

All notable changes to Codex Provider Hub are documented here. The project follows semantic versioning from the first public preview onward.

## [0.1.0] - 2026-08-27

### Added

- Native macOS menu-bar control center for a local Sub2API gateway
- Start, stop, refresh, and gateway health visibility
- OpenAI-compatible provider onboarding, model probing, and Codex catalog synchronization
- Authorized OpenAI/Codex OAuth account imports with per-account quota windows
- Optional ChatGPT model-picker guard for local `use_hidden_models` state
- AIHub balance and daily-usage visibility
- Cursor multi-account usage view with encrypted local token storage
- Transparent, frameless native liquid-glass workspace with dark and light themes
- English, Simplified Chinese, and Japanese documentation
- Automated frontend and native macOS CI
- Automated Apple Silicon and Intel GitHub prereleases with checksums

### Security

- API keys are not hardcoded in the repository
- Custom AIHub and Cursor credentials are encrypted at rest with AES-256-GCM
- Configuration and model-catalog files are backed up before modification

### Distribution note

The 0.1.0 preview uses ad-hoc macOS signing and is not Apple-notarized. Users may need to approve the app in macOS Privacy & Security. Developer ID signing and notarization remain planned.
