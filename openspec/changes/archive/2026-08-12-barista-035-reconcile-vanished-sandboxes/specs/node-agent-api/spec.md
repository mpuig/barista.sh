# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: Reconciliation reconciles a RUNNING instance whose sandbox has vanished

The reconciler SHALL keep the journal consistent with the substrate in **both**
directions. In addition to reaping substrate objects the journal does not know
(barista-034), it SHALL reconcile a **`RUNNING`** instance whose substrate sandbox
is absent to **`FAILED`**, with a degradation event naming the vanished sandbox,
so the node can never report a session as running when its sandbox is gone. A
`FAILED` instance is terminal, so its credential then becomes reapable by the
credential sweep.

To avoid failing a live session on a transient substrate hiccup, the reconciler:

- SHALL act only on a **successful** sandbox enumeration — an enumeration error is
  read as "nothing to reconcile", never as an empty inventory;
- SHALL reconcile a `RUNNING` instance only after its sandbox has been absent
  across a **bounded number of consecutive successful** enumerations, so a single
  missing enumeration cannot mass-fail running instances; and
- SHALL run this reconciliation only for a runtime that **enumerates sandboxes**,
  so a runtime whose transport carries no sandbox inventory (and therefore reports
  none by construction) never has its instances reconciled as vanished.

#### Scenario: a running instance whose sandbox has vanished becomes FAILED
- **WHEN** a `RUNNING` instance's substrate sandbox is absent across the debounce
  window of successful enumerations
- **THEN** the reconciler sets the instance to `FAILED`, emits a degradation naming
  the vanished sandbox, and the instance's credential becomes reapable

#### Scenario: a transient enumeration failure fails no one
- **WHEN** the sandbox enumeration errors on a pass (the substrate is briefly
  unreachable)
- **THEN** no instance is reconciled to `FAILED` on that pass

#### Scenario: a present sandbox leaves the instance untouched
- **WHEN** a `RUNNING` instance's sandbox is present in the enumeration
- **THEN** its state is unchanged and its absence count is reset to zero

#### Scenario: a non-enumerating runtime reconciles nothing
- **WHEN** the runtime reports no sandbox inventory by construction (a runtime with
  no substrate leak surface, e.g. the in-process or `fake` runtimes)
- **THEN** no instance is reconciled as vanished, regardless of journal state
