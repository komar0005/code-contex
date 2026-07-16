# Security Policy

## Scope

This app reads local files written by Claude Code and opencode, and makes
exactly two kinds of network requests: Anthropic's OAuth usage endpoint
(with the token Claude Code already stores on your machine) and LiteLLM's
public price table. It never writes credentials, never sends usage data
anywhere, and has no server component.

## Reporting a vulnerability

Please use GitHub's
[private vulnerability reporting](../../security/advisories/new) instead of
opening a public issue. You should get a response within a week.
