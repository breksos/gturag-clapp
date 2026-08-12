#!/usr/bin/env bash
# Run a command with the one git remote rewrite cargo needs to fetch the clatch crates.
#
#   scripts/with-clatch-deps.sh cargo build --release
#   scripts/with-clatch-deps.sh npm run build
#
# clappkit pins clatch-core/pipe/ipc/registry at ssh://git@github.com/arfium/clatch.git.
# That remote needs a key; the same repository is readable over HTTPS. Rather than asking
# every clone and every CI run to hold a deploy key — PLAYBOOK §8, a dependency you cannot
# reach is not a dependency — rewrite that single URL for the duration of one command.
#
# Deliberately NOT `git config --global`: this touches nothing that outlives the process,
# and it is scoped to one remote, so no other dependency or repository is affected.
set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [args…]" >&2
    exit 2
fi

export CARGO_NET_GIT_FETCH_WITH_CLI=true
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0='url.https://github.com/arfium/clatch.git.insteadOf'
export GIT_CONFIG_VALUE_0='ssh://git@github.com/arfium/clatch.git'

exec "$@"
