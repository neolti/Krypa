#!/usr/bin/env bash
# Runs the web-content scenarios in the booted iOS Simulator and takes a screenshot of each.
# Needs: UDID (booted simulator with the Debug app installed), run from the repo root.
# Signals, not sleeps: the app prints `krypa-ci:` lines to stderr (Debug configuration
# only); this script waits for them with a timeout. A fixed 2 s settle follows only to let
# the last frame reach the screen before the screenshot.
set -u
BUNDLE=app.krypa.ios
OUT=artifacts
mkdir -p "$OUT"
FAILED=0

# wait_for FILE PATTERN SECONDS: 0 when the pattern shows up in FILE in time.
wait_for() {
  local file=$1 pattern=$2 seconds=$3 i
  for ((i = 0; i < seconds * 2; i++)); do
    if grep -Eq "$pattern" "$file" 2>/dev/null; then return 0; fi
    sleep 0.5
  done
  return 1
}

# launch NAME [URL]: fresh process, stderr to $OUT/NAME.stderr.txt.
launch() {
  local name=$1 url=${2:-}
  xcrun simctl terminate "$UDID" "$BUNDLE" 2>/dev/null || true
  sleep 1
  : > "$OUT/$name.stderr.txt"
  if [ -n "$url" ]; then
    xcrun simctl launch --stderr="$PWD/$OUT/$name.stderr.txt" "$UDID" "$BUNDLE" -KrypaStartURL "$url" > "$OUT/$name.launch.txt"
  else
    xcrun simctl launch --stderr="$PWD/$OUT/$name.stderr.txt" "$UDID" "$BUNDLE" > "$OUT/$name.launch.txt"
  fi
}

# scenario NAME URL DONE_PATTERN ATTEMPTS REQUIRED_PATTERN...
# Launch, wait for DONE_PATTERN (up to 60 s), then require every REQUIRED_PATTERN in stderr.
scenario() {
  local name=$1 url=$2 done_pattern=$3 attempts=$4
  shift 4
  local attempt ok=1
  for ((attempt = 1; attempt <= attempts; attempt++)); do
    echo "== $name: attempt $attempt of $attempts"
    launch "$name" "$url"
    if wait_for "$OUT/$name.stderr.txt" "$done_pattern" 60; then
      sleep 2
      ok=0
      local req
      for req in "$@"; do
        if ! grep -Eq "$req" "$OUT/$name.stderr.txt"; then
          echo "::warning::$name: expected line not found: $req"
          ok=1
        fi
      done
      if [ $ok -eq 0 ]; then break; fi
    else
      echo "::warning::$name: no '$done_pattern' within 60 s (network problem or app hang)"
    fi
  done
  xcrun simctl io "$UDID" screenshot "$OUT/$name.png" || true
  if [ -n "${LAST_PLACEMENT:-}" ]; then
    local last
    last=$(grep -E "krypa-ci: placement" "$OUT/$name.stderr.txt" | tail -n 1)
    if ! printf "%s" "$last" | grep -Eq "$LAST_PLACEMENT"; then
      echo "::warning::$name: last placement line is [$last], wanted $LAST_PLACEMENT"
      ok=1
    fi
  fi
  if [ $ok -ne 0 ]; then
    echo "::error::scenario $name failed after $attempts attempt(s); last markers:"
    grep -E "krypa" "$OUT/$name.stderr.txt" | tail -n 15
    FAILED=1
  else
    echo "scenario $name ok:"
    grep -E "krypa-ci" "$OUT/$name.stderr.txt"
  fi
}

# Local fixtures: deterministic, no internet. 8080 plain http, 8443 self-signed https.
scenario http-local "http://127.0.0.1:8080/basic/" 'krypa-ci: finished|krypa-ci: load-failed' 2 \
  'krypa-ci: committed trust=Absent' 'krypa-ci: finished' 'krypa-ci: placement session=1 '
LAST_PLACEMENT='placement none$' scenario cert-error "https://127.0.0.1:8443/basic/" 'krypa-ci: load-failed' 2 \
  'krypa-ci: load-failed domain=NSURLErrorDomain code=-120[0-6]'
# The one scenario that needs the internet: three attempts, each a fresh launch with a 60 s
# budget. If example.com is unreachable from the runner the job fails with that message
# instead of passing silently.
scenario https-example "https://example.com/" 'krypa-ci: finished|krypa-ci: load-failed' 3 \
  'krypa-ci: committed trust=Valid only-secure=true' 'krypa-ci: finished' 'krypa-ci: placement session=1 '

exit $FAILED
