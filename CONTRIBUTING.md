# Contributing to Codex Provider Hub

Thank you for considering a contribution. The project is early-stage, so focused bug reports, reproducible compatibility findings, documentation improvements, and small well-tested pull requests are especially valuable.

## Before you start

- Search existing issues and pull requests to avoid duplicate work.
- Open an issue before a large architectural change.
- Never include API keys, OAuth tokens, JWTs, `.env` files, account exports, or unredacted logs.
- Use only accounts and providers you own or are authorized to manage while testing.

Security-sensitive reports belong in [SECURITY.md](SECURITY.md), not in a public issue.

## Development setup

Requirements:

- macOS 11+
- Node.js 20+
- Rust stable
- A local Sub2API installation for integration testing

```bash
git clone https://github.com/ningsam/codex-provider-hub.git
cd codex-provider-hub
npm install
export SUB2API_DIR="$HOME/path/to/your/sub2api-ready"
npm run tauri dev
```

Frontend validation:

```bash
npm run build
```

Desktop build:

```bash
npm run tauri build
```

## Good contribution areas

- English, Simplified Chinese, and Japanese localization
- macOS packaging, signing, notarization, and release automation
- Provider compatibility and clearer error reporting
- Accessibility, keyboard navigation, and reduced-motion behavior
- Documentation, screenshots, and short reproducible examples
- Tests for quota parsing, model mapping, and configuration backup behavior

## Pull request expectations

1. Keep the change focused and explain the user problem it solves.
2. Preserve the local-first security model.
3. Add or update tests when behavior changes.
4. Run `npm run build` and the relevant Rust checks locally.
5. Include before/after screenshots for visible UI changes.
6. Update all affected README translations when changing user-facing documentation.
7. Clearly identify anything you could not test.

The pull request template contains the final checklist.

## Style

- Use existing TypeScript and Rust patterns before introducing new abstractions.
- Prefer explicit error messages over silent fallbacks.
- Avoid adding dependencies when a small local implementation is sufficient.
- Keep interface copy concise and translatable.

## Commit messages

Conventional-style prefixes are encouraged:

```text
feat: add provider health history
fix: preserve catalog backup on sync failure
docs: clarify Sub2API setup
chore: update build tooling
```

By contributing, you agree that your contribution will be licensed under the repository's [MIT License](LICENSE).
