# Design: startup orphan scan

Committed local objects are atomic files named by digest. At startup no export executor is active, so after operation recovery it is safe to compare those files with every digest known to the reference journal and remove unknown files. Running the same scan during normal GC would race the intentional commit-before-registration window, so the orphan scan is startup-only. Remote CAS reclamation remains governed by bucket lifecycle.
