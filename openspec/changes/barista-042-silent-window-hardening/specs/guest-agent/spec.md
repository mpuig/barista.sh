# guest-agent — Delta Specification

## ADDED Requirements

### Requirement: A WriteFile stream that stops making progress is ended

The guest agent SHALL bound the gap between consecutive frames of a `WriteFile`
stream. When no frame arrives within the bound, it SHALL fail the RPC with an
explicit status (`DEADLINE_EXCEEDED`) whose message says the stream went quiet
and states the per-frame-gap rule — releasing the RPC and the open file handle
on the guest, and thereby the host relaying it — rather than holding both open
indefinitely for a client that opened a write and stopped sending.

The bound SHALL apply to the gap between frames, never to the upload's total
size or total duration: a stream that keeps sending chunks SHALL never be ended
by it, however large or slow the upload. A size cap is explicitly not part of
this requirement — the sandbox's own disk budget already bounds the bytes, and
ENOSPC reports the overrun with the filesystem's authority.

A partial file MAY remain after the abort, containing exactly the bytes
received before the stream went quiet. This is the same contract a mid-stream
transport failure has always left behind; the bound adds no new failure shape,
it converts an unbounded hold into that existing one.

`Exec` is deliberately outside this requirement: an interactive session is
legitimately idle for long stretches, and its stream endings (half-close,
transport break) are already handled explicitly.

#### Scenario: a quiet write stream is ended, and says why
- **WHEN** a client opens a `WriteFile` stream (with or without some chunks)
  and then sends no further frame for the bound
- **THEN** the RPC fails with `DEADLINE_EXCEEDED`, the message says the stream
  went quiet and names the per-frame-gap rule, and the file handle is released
  with the bytes received so far on disk

#### Scenario: a slow but progressing upload is never ended
- **WHEN** every gap between consecutive frames of a `WriteFile` stream stays
  within the bound
- **THEN** the upload completes normally and reports its full `bytes_written`,
  regardless of the upload's total size or total duration

#### Scenario: the happy path is unchanged
- **WHEN** a client streams `open` followed by chunks and closes its half
- **THEN** the file lands byte-identical with the requested mode and the
  response reports the exact `bytes_written`, exactly as before this
  requirement
