# Design: bounded session application logs

## D1 — Proxy the substrate, do not create a second log store

`Runtime` gains a read-only application-log stream. The Hypeman implementation requests `GET /instances/{id}/logs?source=app&tail=N&follow=…` with the existing bearer-authenticated client and parses its SSE JSON-string data frames. The node neither opens Hypeman's private files nor persists copied bytes.

Rejected: reading `/var/lib/hypeman/guests/*/logs/app.log`. That depends on private layout and violates the adopted-substrate boundary.

Rejected: projecting lines into `WatchEvents`. Logs and lifecycle facts have different retention and cursor semantics, and a chatty workload must not consume the operation journal's retention budget.

## D2 — Additive byte-preserving Contract A stream

`WatchLogs(WatchLogsRequest) returns (stream LogEntry)` is additive. The request names one instance, a historical line count, and follow mode. `tail=0` means the documented default of 100; values above 1000 are invalid. Each `LogEntry.data` is one substrate-delimited application-log line encoded as bytes. No timestamp is invented because the substrate does not supply one separately from line content.

A stream can end normally when `follow=false` or when the substrate closes it. It can fail explicitly; it must never silently switch to another log source.

## D3 — Application logs only

The node hard-codes `source=app`. VMM logs can contain host topology and substrate details; Hypeman operational logs belong to the node operator. Neither is tenant workload output and neither enters this contract.

## D4 — Backpressure and bounds

The reqwest body is consumed incrementally. The SSE parser holds only one bounded frame and rejects a data frame above 256 KiB. The channel to gRPC is bounded, so a slow reader applies backpressure instead of growing memory without limit. Historical volume is bounded by `tail <= 1000`; follow mode remains naturally open-ended and caller-cancellable.

## D5 — Fake runtime remains honest

The fake runtime stores bounded test log lines per instance and returns them through the same runtime seam. It does not claim substrate behavior such as rotation. Contract and CLI tests use this seam; the Hypeman parser has focused SSE fragmentation and malformed-frame tests.
