# Design: remote lifecycle ownership

The remote store is a shared immutable CAS. Node-local capsule references are not a global liveness oracle: another node or installation may still name the same digest. Consequently node deletion cannot safely call remote removal. Local registration and local cache bytes are node-owned; remote retention, delayed reclamation, and erasure are bucket-policy/operator responsibilities. This is explicit rather than pretending node deletion provides global erasure.
