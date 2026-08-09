# POST /instances answers a bare 500 for an unsupported hypervisor

**Versions:** hypeman-api 0.17.0 (Linux), API 0.3.0.

Requesting `hypervisor: vz` on a Linux host fails with an unexplained
`500 internal_error`. The real message — `no VM starter for hypervisor type: vz`
— is only in the daemon's journal.

An unsupported hypervisor for this host is a client error with a knowable
cause: a `400` (or `422`) carrying that exact string would let an API consumer
distinguish "I asked for something this host cannot do" from "the daemon is
broken". As is, every programmatic caller has to treat them the same.
