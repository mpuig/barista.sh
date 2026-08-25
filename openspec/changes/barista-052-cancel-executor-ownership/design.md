# Design: executor ownership after cancellation

`Operation.state` remains the client-visible outcome. A new journal bit, `executor_active`, answers the distinct internal question of whether substrate work can still mutate the instance. Submission sets it; final executor cleanup clears it even when cancellation has already made the public state terminal. Conflict checks and fork-source reconciliation use this bit. This preserves the documented non-interrupting cancellation behavior without weakening single-writer instance mutation.
