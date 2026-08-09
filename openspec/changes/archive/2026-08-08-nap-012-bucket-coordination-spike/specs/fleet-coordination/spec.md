# fleet-coordination — Delta Specification

## Purpose

The obligations any Phase 2 coordination layer must meet for Nap's session
model: exactly-one-owner for named, single-writer sessions; safe failover; and
name-based addressing — written from what the session model requires, so the
requirements hold whichever mechanism ADR-002 ratifies.

## ADDED Requirements

### Requirement: Exactly one owner per session name
The coordination layer SHALL guarantee that at most one node owns a session
name at any time, including under concurrent acquisition attempts, and SHALL
make ownership transitions explicit (an epoch or equivalent monotonic token)
so that a node whose ownership lapsed can be rejected deterministically.

#### Scenario: contended acquisition yields one owner
- **WHEN** two nodes attempt to acquire the same unowned session name
  concurrently
- **THEN** exactly one acquisition succeeds, the other observes a conflict it
  can distinguish from an error, and the epoch advances exactly once

#### Scenario: a stale owner cannot act
- **WHEN** a node holding an expired lease attempts a mutation fenced by its
  old epoch
- **THEN** the mutation is rejected, regardless of the node's own clock

### Requirement: The name resolves to the owner
Resolving a session name SHALL return its current owner and enough to reach
the session, using the same authoritative record that ownership acquisition
writes — addressing and coordination SHALL NOT maintain separate tables that
can disagree.

#### Scenario: resolve-then-reach
- **WHEN** a client resolves a session name while the session is owned
- **THEN** the answer identifies the owning node from the same record a
  competing acquirer would contend on

### Requirement: Coordination unavailability is explicit and non-destructive
While the coordination backend is unreachable, a node SHALL keep already-owned
sessions running, SHALL refuse new acquisitions with an explicit reason, and
SHALL NOT release, destroy, or re-acquire anything on the strength of the
outage alone.

#### Scenario: outage does not orphan or duplicate
- **WHEN** the coordination backend becomes unreachable while sessions run
- **THEN** running sessions continue undisturbed, new acquisitions fail with a
  machine-readable reason, and no second owner can emerge during the outage

### Requirement: A single node needs no coordination backend
A node operating alone SHALL provide the full session lifecycle without any
coordination backend configured; coordination SHALL be required only where a
second node could contend for the same names.

#### Scenario: laptop mode
- **WHEN** a node runs with no coordination backend configured
- **THEN** every lifecycle verb works exactly as in Phase 1, and nothing
  reports degradation
