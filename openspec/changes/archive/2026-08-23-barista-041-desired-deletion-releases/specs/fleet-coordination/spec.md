# Delta for fleet-coordination — barista-041-desired-deletion-releases

## ADDED Requirements

### Requirement: Deleting the desired record releases the name

When a successful listing of `desired/` does not contain a name this node
owns, the node SHALL converge that name to released: it SHALL tear down the
local instance through ordinary journaled operations, and SHALL release the
lease — a fenced, expiry-zeroing write, never an object deletion — only after
the teardown is observed complete in its journal. Until then it SHALL keep
renewing the lease, so no other node can acquire a name whose workload still
runs.

The deletion signal SHALL be the absence of the record from the listing's
keys: a record that exists but cannot be read SHALL count as desired, and a
listing that fails SHALL release and destroy nothing (coordination
unavailability stays non-destructive). A lease currently being fenced SHALL
be left to the fencing path.

The obligation SHALL survive a restart: a journaled lease the bucket still
shows as this node's, for a name no longer desired, SHALL be re-acquired and
converged the same way rather than left running unowned.

A release refused by the backend — the node was superseded — SHALL be treated
as success for the release itself: the name is not this node's either way,
and the write's refusal is what protects the new owner's record.

#### Scenario: a deleted name is torn down and freed

- **WHEN** a consumer deletes `desired/<name>` while its owner runs the
  session
- **THEN** within bounded passes the owner destroys the instance, releases
  the lease with its epoch intact, and another acquirer can take the name
  without waiting out a TTL

#### Scenario: a wedged name with no live instance frees immediately

- **WHEN** the owner holds a lease whose desired record is gone and whose
  instance the journal does not know as live
- **THEN** the next pass releases the lease without waiting on any teardown

#### Scenario: an unreadable record is not a deleted record

- **WHEN** `desired/<name>` exists but cannot be parsed
- **THEN** the owner keeps the lease and the workload untouched, exactly as
  it does for a record it cannot act on

#### Scenario: an outage deletes nothing

- **WHEN** the bucket cannot be listed while this node owns names
- **THEN** no lease is released and no instance is destroyed on the strength
  of the outage

#### Scenario: a restarted owner still honours the deletion

- **WHEN** a desired record is deleted while its owner is down, and the owner
  restarts with the workload still running
- **THEN** the owner re-acquires its own lease, tears the workload down, and
  releases — the name does not stay consumed and the workload does not run
  unowned
