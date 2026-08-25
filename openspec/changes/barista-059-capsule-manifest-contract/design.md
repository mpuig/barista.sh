# Design: capsule manifest v1alpha2

The schema adds explicit architecture, creation timestamp, required restore capability names, and object media types. Export derives creation time from the retained snapshot so repeated exports remain content-stable. Canonical identity length-prefixes all new scalar values, sorts capability names, and includes media type in each sorted object tuple. Import rejects missing, unknown, incompatible, or inconsistent values before registration; restore checks architecture explicitly.
