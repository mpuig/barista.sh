---
name: Bug report
about: Something Barista did that it should not have
title: ""
labels: bug
assignees: ""
---

<!--
Not for security vulnerabilities — see SECURITY.md and report privately instead.
-->

## What happened

A clear description of the behaviour, and what you expected instead.

## Reproduction

Steps, or ideally the exact commands. The scenario runner and `barista --json`
make for reproducible reports.

```
# commands / spec here
```

## Environment

- **Runtime**: `fake` / `hypeman` / `runsc` (which, and version)
- **Tier**: A (full host) / B (cluster w/ node access) / C (serverless) — see BRD §1
- **Single node or fleet** (and the bucket backend, if a fleet)
- **Host OS / arch**:
- **Barista commit**:

## Capability context

If it involves a capability that degraded or was refused, paste the relevant
`RuntimeCapabilities` / `CAPABILITY_MISSING` / `Snapshot.kind` output — Barista is
meant to be honest about what a host grants, so that output is usually the clue.

## `make check`

Does `make check` pass on your checkout? If the bug shows up there, paste the
failing output.

## Anything else

Logs, events (`WatchEvents`), a journal excerpt, or a hypothesis.
