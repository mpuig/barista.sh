# Fail-closed local definition of done.
# Copy project.mk.example to project.mk and set CHECK_COMMAND when the workspace exists (nap-001).

-include project.mk

.PHONY: check openspec-check project-check

check: openspec-check project-check

openspec-check:
	@command -v openspec >/dev/null 2>&1 || { echo "OpenSpec is not installed"; exit 2; }
	@openspec validate --all --strict

project-check:
	@test -n "$(strip $(CHECK_COMMAND))" || { \
		echo "CHECK_COMMAND is not configured. Copy project.mk.example to project.mk and set it."; \
		exit 2; \
	}
	@$(CHECK_COMMAND)
