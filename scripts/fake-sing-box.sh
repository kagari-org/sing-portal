#!/bin/sh
# Stand-in for the real sing-box binary during local runs.
# Emulates the api subcommands the agent uses: version, group show/select/urltest,
# outbounds (with a delay that changes on every call) and networkquality.
# Set FAKE_SING_BOX_FAIL=1 to simulate a rejected config.

state="${FAKE_SING_BOX_STATE:?}"
mkdir -p "$state"

if [ "${FAKE_SING_BOX_FAIL}" = "1" ]; then
  echo "FATAL decode config at path: unknown field \"bogus\"" >&2
  exit 1
fi

case "$1 $2 $3" in
  "api group show")
    echo "Tag:       s_out"
    echo "Type:      selector"
    echo "Selected:  $(cat "$state/selection" 2>/dev/null || echo s_auto)"
    ;;
  "api group select")
    echo "$5" >"$state/selection"
    ;;
  "api group urltest")
    ;;
  "api outbounds"*)
    counter="$state/counter"
    value=$(($(cat "$counter" 2>/dev/null || echo 40) + 1))
    echo "$value" >"$counter"
    if [ -f "$state/tags" ]; then
      while IFS= read -r tag; do
        [ -n "$tag" ] && printf '%s\tdirect\t%s ms\n' "$tag" "$value"
      done <"$state/tags"
    fi
    printf 's_out\tselector\t%s ms\n' "$value"
    printf 's_auto\turltest\t%s ms\n' "$value"
    ;;
  "api networkquality"*)
    cat <<'EOF'
==== NETWORK QUALITY TEST ====
Idle Latency: 12 ms Accuracy: High
Download Capacity: 88.0 Mbps Accuracy: High
Upload Capacity: 42.0 Mbps Accuracy: High
Download Responsiveness: 600 RPM Accuracy: Medium
Upload Responsiveness: 520 RPM Accuracy: Medium
EOF
    ;;
  "api version"*)
    echo "1.14.3"
    ;;
esac

exit 0
