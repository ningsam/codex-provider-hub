# Security Policy

Codex Provider Hub handles local API keys, OAuth material, account metadata, and configuration files. Please treat security reports carefully.

## Supported code

Security fixes are made against the latest `main` branch and the most recent published release when releases are available.

## Reporting a vulnerability

Use GitHub's **Security** tab and choose **Report a vulnerability** when private vulnerability reporting is available.

If that option is unavailable, open a minimal public issue asking the maintainer to establish a private contact channel. Do not include exploit details or sensitive data in that issue.

## Never include

- API keys or authorization headers
- OAuth access or refresh tokens
- Cursor JWTs
- Raw account export files
- Unredacted `.env`, database, LevelDB, or application-state files
- Logs containing credentials, cookies, email addresses, or local filesystem details that are not required to reproduce the issue

Use synthetic values and redact secrets consistently. When a path is relevant, replace personal directories with placeholders such as `$HOME`.

## Scope examples

Reports are especially useful when they involve:

- Credential exposure or insecure persistence
- Incorrect encryption or key handling
- Unsafe command execution or path handling
- Configuration backup/restore failures that could leak or destroy data
- Unintended network requests that transmit secrets
- Privilege escalation or sandbox boundary issues

General bugs and feature requests should use the normal issue templates.
