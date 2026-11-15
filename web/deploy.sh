#!/bin/sh
# The browser emulator is published as part of the wiki at rns-e.com, under
# /emulator/: build with wiki/build.sh and publish with wiki/deploy.sh.
exec "$(dirname "$0")/../wiki/deploy.sh" "$@"
