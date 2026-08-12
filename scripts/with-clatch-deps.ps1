# Run a command with the one git remote rewrite cargo needs to fetch the clatch crates.
#
#   .\scripts\with-clatch-deps.ps1 cargo build --release
#   .\scripts\with-clatch-deps.ps1 npm run build
#
# clappkit pins clatch-core/pipe/ipc/registry at ssh://git@github.com/arfium/clatch.git.
# That remote needs a key; the same repository is readable over HTTPS. Rather than asking
# every clone and every CI run to hold a deploy key — PLAYBOOK §8, a dependency you cannot
# reach is not a dependency — rewrite that single URL for the duration of one command.
#
# Deliberately NOT `git config --global`: this touches nothing that outlives the process.
# It is scoped to one remote, so no other dependency, repository or tool is affected.

param(
    [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
    [string[]] $Command
)

$ErrorActionPreference = 'Stop'

$env:CARGO_NET_GIT_FETCH_WITH_CLI = 'true'
$env:GIT_CONFIG_COUNT = '1'
$env:GIT_CONFIG_KEY_0 = 'url.https://github.com/arfium/clatch.git.insteadOf'
$env:GIT_CONFIG_VALUE_0 = 'ssh://git@github.com/arfium/clatch.git'

$exe = $Command[0]
$rest = @()
if ($Command.Length -gt 1) { $rest = $Command[1..($Command.Length - 1)] }

& $exe @rest
exit $LASTEXITCODE
