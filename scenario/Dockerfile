# T7's workload image: an ACP-shaped agent session whose in-memory conversation
# context must survive a pause and resume (spec §9 T7, constitution v1.1.0).
#
# **Pinned by digest, not by tag.** A snapshot's restore key is derived from the
# image that produced it (B29), so a tag repointed at different bytes would
# silently invalidate every snapshot taken from it — and the failure would appear
# as a mysterious cold-boot fallback long after the change. `python:3.13-alpine`
# at the digest below; update both together.
FROM python:3.13-alpine@sha256:9b4929a72599b6c6389ece4ecbf415fd1355129f22bb92bb137eea098f05e975

# No pip install: the session deliberately has no dependencies. Every package is
# something that could fail to install at image-build time and something whose
# own state might not survive a snapshot — neither is what T7 is testing.
COPY acp_session.py /usr/local/bin/acp-session
RUN chmod 0755 /usr/local/bin/acp-session

# Unbuffered: the session replies line by line to a caller that is waiting, and
# Python's default block buffering on a pipe would hold those replies until exit.
ENV PYTHONUNBUFFERED=1

# Barista overrides the entrypoint with its guest agent and takes the workload from
# `spec.process.start_cmd`, so this is documentation of intent plus a working
# `docker run` for anyone testing the image on its own.
ENTRYPOINT ["/usr/local/bin/acp-session"]
