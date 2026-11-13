#!/usr/bin/env bash
set -euo pipefail

SCRIPT_NAME="$(basename "$0")"
STATE_FILE="${RUNNER_TEMP:-/tmp}/previous-release"

usage() {
  cat <<EOF
Guard a CD deploy: remember the running release, smoke-check the public site, and restore on failure.

Usage:
  ${SCRIPT_NAME} record WORKLOAD...        Remember deployments and cronjobs, e.g. deployment/app cronjob/worker
  ${SCRIPT_NAME} smoke CODE URL [CODE URL]...  Expect each URL to answer with its HTTP status code
  ${SCRIPT_NAME} restore                    Put back the release saved by record

Requires NAMESPACE and a configured kubectl.
EOF
}

# Save the deployment revision or the cronjob container images that run before this deploy.
record() {
  : >"${STATE_FILE}"
  local ref value
  for ref in "$@"; do
    case "${ref%%/*}" in
      deployment)
        value="$(kubectl -n "${NAMESPACE}" get "${ref}" --ignore-not-found \
          -o jsonpath='{.metadata.annotations.deployment\.kubernetes\.io/revision}')"
        ;;
      cronjob)
        value="$(kubectl -n "${NAMESPACE}" get "${ref}" --ignore-not-found \
          -o jsonpath='{range .spec.jobTemplate.spec.template.spec.containers[*]}{.name}={.image} {end}')"
        ;;
      *)
        echo "Unsupported workload: ${ref}" >&2
        exit 2
        ;;
    esac
    echo "${ref} ${value}" >>"${STATE_FILE}"
    echo "Recorded ${ref}: ${value:-not deployed yet}"
  done
}

# Retry each URL briefly, since Traefik can route to the old pods for a few seconds after a rollout.
smoke() {
  local failed=0 expected url code attempt
  while [ "$#" -ge 2 ]; do
    expected="$1"
    url="$2"
    shift 2
    for attempt in 1 2 3 4 5 6; do
      code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 15 "${url}" || true)"
      [ "${code}" = "${expected}" ] && break
      [ "${attempt}" -lt 6 ] && sleep 10
    done
    if [ "${code}" = "${expected}" ]; then
      echo "OK   ${code} ${url}"
    else
      echo "FAIL ${code} ${url} (expected ${expected})" >&2
      failed=1
    fi
  done
  return "${failed}"
}

# Roll workloads back to the recorded release, skipping those that were new or unchanged.
restore() {
  if [ ! -s "${STATE_FILE}" ]; then
    echo "No recorded release, nothing to restore."
    return 0
  fi
  local ref value current
  while read -r ref value; do
    if [ -z "${value}" ]; then
      echo "Skip ${ref}: it did not exist before this deploy."
      continue
    fi
    case "${ref%%/*}" in
      deployment)
        current="$(kubectl -n "${NAMESPACE}" get "${ref}" \
          -o jsonpath='{.metadata.annotations.deployment\.kubernetes\.io/revision}')"
        if [ "${current}" = "${value}" ]; then
          echo "Skip ${ref}: still on revision ${value}."
          continue
        fi
        kubectl -n "${NAMESPACE}" rollout undo "${ref}" --to-revision="${value}"
        kubectl -n "${NAMESPACE}" rollout status "${ref}" --timeout=300s
        ;;
      cronjob)
        # shellcheck disable=SC2086 # value holds space-separated container=image pairs.
        kubectl -n "${NAMESPACE}" set image "${ref}" ${value}
        ;;
    esac
  done <"${STATE_FILE}"
}

: "${NAMESPACE:?NAMESPACE is required}"
command="${1:-}"
shift || true
case "${command}" in
  record) record "$@" ;;
  smoke) smoke "$@" ;;
  restore) restore ;;
  *)
    usage >&2
    exit 2
    ;;
esac
