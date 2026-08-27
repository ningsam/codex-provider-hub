# Codex Provider Hub Roadmap

This roadmap communicates direction rather than fixed delivery dates. Priorities may change based on real usage, upstream compatibility, security, and contributor capacity.

## Current foundation

- Local Sub2API gateway start/stop and health visibility
- OpenAI-compatible provider onboarding and model catalog synchronization
- Authorized OpenAI/Codex OAuth account import and quota display
- Optional ChatGPT model picker guard
- AIHub balance visibility
- Cursor account usage view
- Native macOS menu-bar workspace
- Responsive liquid-glass interface

## Next: distribution and onboarding

- Downloadable macOS release artifacts
- Apple signing and notarization path
- First-run setup wizard for locating or validating Sub2API
- Clear diagnostics export with automatic secret redaction
- Release notes and upgrade guidance
- Homebrew Cask evaluation after stable releases exist

## Next: internationalization and daily use

- In-app language switch for English, Simplified Chinese, and Japanese
- Persisted layout and refresh preferences
- Provider health history and clearer failure attribution
- Safer import/export of non-secret configuration
- Improved empty states and guided recovery actions

## Later / under evaluation

- Auto-update support
- Extensible provider adapters
- Windows and Linux feasibility assessment
- More granular routing policies and model aliases
- Optional local notifications for low quota or unhealthy providers

## Explicitly not a goal

- A hosted credential vault
- Bypassing provider quotas, authentication, or platform terms
- Selling, sharing, or brokering third-party accounts

Suggestions are welcome through the repository's feature-request template. A proposal should explain the user problem, the local-first security impact, and how it can be tested.
