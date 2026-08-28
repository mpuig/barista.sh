## ADDED Requirements

### Requirement: The CLI SHALL expose bounded application logs

The CLI SHALL provide `barista logs <instance>` with a historical tail bound from 1 through 1000 and optional follow mode. Human output SHALL preserve each application-log entry as a line. JSON output SHALL preserve exact bytes through base64 rather than lossy text conversion.

#### Scenario: operator reads startup diagnostics

- **WHEN** an operator runs `barista logs --tail 100 <instance>`
- **THEN** the CLI prints the bounded application-log history and exits when history completes

#### Scenario: automation reads non-UTF-8 output

- **WHEN** `barista --json logs <instance>` receives arbitrary bytes
- **THEN** each JSON line contains those exact bytes as `data_base64`
