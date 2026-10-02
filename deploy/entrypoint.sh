#!/bin/sh
# Selects which example company this container runs, from $OPENCOMPANY_COMPANY.
# The value may be an example directory name (e.g. venture_capital) or a
# friendly alias (e.g. fund). This is the "which module spins up" switch.
set -eu

COMPANY="${OPENCOMPANY_COMPANY:-marketing_agency}"

# Friendly aliases → example directory names.
case "$COMPANY" in
  fund | vc | venture-capital)     COMPANY="venture_capital" ;;
  marketing | agency)              COMPANY="marketing_agency" ;;
  software | saas | dev)           COMPANY="software_company" ;;
  studio | venture-studio)         COMPANY="venture_studio" ;;
  accelerator)                     COMPANY="startup_accelerator" ;;
  law | legal)                     COMPANY="law_firm" ;;
  accounting | finance)            COMPANY="accounting_firm" ;;
  support)                         COMPANY="customer_support" ;;
  signals | opportunity)           COMPANY="signals_opportunity_studio" ;;
esac

DIR="companies/${COMPANY}"
if [ ! -f "${DIR}/company.toml" ] && [ ! -f "${DIR}/agents.toml" ]; then
  echo "opencompany: unknown company '${OPENCOMPANY_COMPANY}' (no manifest at ${DIR})" >&2
  echo "available companies:" >&2
  ls companies | sed 's/^/  - /' >&2
  exit 1
fi

DISCOVER=""
if [ "${OPENCOMPANY_DISCOVERABLE:-false}" = "true" ]; then
  DISCOVER="--discoverable"
fi

# The per-turn wall-clock ceiling the vendored harness enforces (issue #1680).
#
# Exported here rather than resolved in-process on purpose. Two reasons:
#
#   1. A turn that hits this ceiling cannot be recognised from its error -- the
#      harness replaces a failed hosted invocation with a fixed sentence, so the
#      wall-clock leaf never arrives. What OpenCompany can do is compare the
#      turn's measured duration against the ceiling, and that needs the ceiling
#      to be a number this side knows. Declaring it here is what makes the pause
#      (rather than a hard run failure) possible at all.
#   2. `set_var` inside a running process races every concurrent `getenv` and is
#      undefined behaviour on glibc -- see the hazard note in
#      `crates/opencompany-core/src/app/boot.rs`. A variable exported before
#      `exec` has no such problem.
#
# Operator-overridable, and `0` restores the vendored "no ceiling" behaviour
# (which also disables the duration-based detection, by design: with no ceiling
# there is nothing to measure against). The vendored default is deliberately NOT
# mirrored here -- it moved from 600 to 3600 in the #2466 bump without anything
# failing, which is the drift this export exists to stop depending on.
export OPENHUMAN_AGENT_TURN_TIMEOUT_SECS="${OPENHUMAN_AGENT_TURN_TIMEOUT_SECS:-600}"

echo "opencompany: launching '${COMPANY}' on ${OPENCOMPANY_BIND:-0.0.0.0:8080}" \
  "(turn ceiling ${OPENHUMAN_AGENT_TURN_TIMEOUT_SECS}s)"
# shellcheck disable=SC2086
exec opencompany serve \
  --company "${DIR}" \
  --bind "${OPENCOMPANY_BIND:-0.0.0.0:8080}" \
  --home "${OPENCOMPANY_DATA_DIR:-/data}" \
  ${DISCOVER}
