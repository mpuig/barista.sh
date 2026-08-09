# Ingress DNS binds :5353 and collides with systemd-resolved on Ubuntu 24.04

**Versions:** hypeman-api 0.17.0 (Linux). macOS installs pick 5354 already.

On Ubuntu 24.04, `systemd-resolved` holds `0.0.0.0:5353` for mDNS. hypeman's
ingress DNS wants the same port and crash-loops on `address already in use` —
silently, from the operator's point of view, because nothing at install time
says so.

Since the macOS install already defaults to 5354, aligning the Linux default
(or probing the port at install and picking a free one, with a line of output)
would remove the failure entirely.

Workaround: `sed -i "s/^  internal_dns_port: 5353/  internal_dns_port: 5354/" /etc/hypeman/config.yaml`
before first start.
