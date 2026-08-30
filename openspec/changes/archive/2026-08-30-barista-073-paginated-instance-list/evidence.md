# Evidence: Bounded instance inventory

## Automated verification

PR #85 passed the complete Ubuntu and macOS definitions of done plus the beta toolchain build. `buf breaking` accepted the additive fields. The server tests traversed filtered two-row pages without gaps or repeats, refused a page size above 256, refused a malformed token, and checked the encoded response budget. CLI unit tests and strict documentation/OpenSpec builds passed.

## Managed verification

Deployed clean remote `main` revision `154725c51182cf42be708b74a1c9a38a6b51b6e1` to the managed node. The previous node-agent and CLI binaries were retained before replacement. The revision marker was written only after the restarted service was active and the new CLI completed `barista doctor` successfully.

The managed journal contained 12,082 retained instances. `barista doctor` followed 48 bounded pages and reported:

```text
12082 instance(s)
```

The prior unary implementation failed above tonic's 4 MiB decode limit on this same inventory. No transport limit was increased. The node, existing Cloud appliance session, generated product service, and public endpoints remained healthy after replacement.
