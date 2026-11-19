#!/bin/sh
# cargo test --release in the current directory for split/*/verify.sh,
# printing the summary lines.  RNSE_LOCAL=DIR builds against that rnse
# checkout instead of the pinned commit (for trying the tooling out
# before the commit is on GitHub); Cargo.toml and the lock are put back.
set -eu
log=$(mktemp)
# Through the session's or CI's proxy cargo's own git client can fail.
export CARGO_NET_GIT_FETCH_WITH_CLI=true
if [ -n "${RNSE_LOCAL:-}" ]; then
    cp Cargo.toml "$log.toml"
    [ -f Cargo.lock ] && cp Cargo.lock "$log.lock"
    sed -i.bak "s#^rnse = { git = .*#rnse = { path = \"$RNSE_LOCAL\" }#" Cargo.toml && rm -f Cargo.toml.bak
fi
status=0
cargo test --release "$@" >"$log" 2>&1 || status=$?
if [ -n "${RNSE_LOCAL:-}" ]; then
    mv "$log.toml" Cargo.toml
    if [ -f "$log.lock" ]; then mv "$log.lock" Cargo.lock; else rm -f Cargo.lock; fi
fi
[ $status = 0 ] || { tail -40 "$log"; rm -f "$log"; echo "FAIL: cargo test" >&2; exit 1; }
grep "^test result" "$log"
grep -E "^warning" "$log" && { rm -f "$log"; echo "FAIL: warnings" >&2; exit 1; }
rm -f "$log"
[ -n "${RNSE_DISC:-}" ] || echo "NOTE: RNSE_DISC unset: the emulator tests returned early and proved nothing"
