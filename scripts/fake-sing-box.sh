#!/bin/sh
# Stand-in for the real sing-box binary during local agent tests.
# Accepts `check -c <path>` and exits 0; set FAKE_SING_BOX_FAIL=1 to simulate a bad config.

if [ "${FAKE_SING_BOX_FAIL}" = "1" ]; then
  echo "FATAL decode config at path: unknown field \"bogus\"" >&2
  exit 1
fi

exit 0
