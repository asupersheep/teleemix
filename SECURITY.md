# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| latest (main) | ✅ |
| dev | ⚠️ Experimental, not recommended for production |
| < 1.0.0 | ❌ |

## Reporting a Vulnerability

If you discover a security vulnerability in Teleemix, please **do not open a public issue**.

Instead, report it privately using one of these methods:

**GitHub Private Vulnerability Reporting** (preferred)
Use the [Report a vulnerability](https://github.com/asupersheep/teleemix/security/advisories/new) button on the Security tab.

**What to include:**
- A description of the vulnerability
- Steps to reproduce it
- The potential impact
- Any suggested fixes if you have them

**What to expect:**
- Acknowledgement within 48 hours
- A fix or mitigation within 14 days for critical issues
- Credit in the release notes if you wish

## Scope

This policy covers the Teleemix bot code and its Docker image. It does not cover:
- Third-party services (Telegram, Deezer, AudD, OpenAI)
- Your own deemix instance
- Your server infrastructure

## Security considerations for deployers

- Keep your `TELEGRAM_TOKEN` and `DEEMIX_ARL` private
- Do not expose your deemix instance publicly without authentication
- Keep your Docker images up to date — use Watchtower or pull regularly
- The Docker socket mount (`/var/run/docker.sock`) gives the container elevated access — only deploy on trusted infrastructure
