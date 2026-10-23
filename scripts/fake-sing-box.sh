#!/bin/sh
# Stand-in for the real sing-box binary during local runs.
# Emulates the subcommands the agent uses: check / tools fetch / tools networkquality / api networkquality.
# Set FAKE_SING_BOX_FAIL=1 to simulate a rejected config.

if [ "${FAKE_SING_BOX_FAIL}" = "1" ]; then
  echo "FATAL decode config at path: unknown field \"bogus\"" >&2
  exit 1
fi

case "$1 $2" in
  "tools fetch")
    exec head -c 2097152 /dev/zero
    ;;
  "tools networkquality"|"api networkquality")
    cat <<'EOF'
==== NETWORK QUALITY TEST ====
Idle Latency: 12 ms Accuracy: High
Download Capacity: 88.0 Mbps Accuracy: High
Upload Capacity: 42.0 Mbps Accuracy: High
Download Responsiveness: 600 RPM Accuracy: Medium
Upload Responsiveness: 520 RPM Accuracy: Medium
EOF
    ;;
esac

exit 0
