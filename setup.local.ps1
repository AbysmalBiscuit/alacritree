<#
.SYNOPSIS
Copy the untracked local files this branch carries into the main checkout.

.DESCRIPTION
AGENTS.local.md, CLAUDE.local.md, devkit.local.toml and the .local.ps1 scripts
are ignored by git, so they are tracked here instead of on a feature branch.
Run this from this branch's worktree to seed a fresh machine, or after pulling
a change to one of them.

A file the main checkout already has is left alone unless it is identical or
-Force is given, so local edits are never silently overwritten.

.EXAMPLE
./setup.local.ps1 -WhatIf
Report what would be copied without writing anything.

.EXAMPLE
./setup.local.ps1 -Force
Overwrite the main checkout's copies with this branch's.
#>
#Requires -Version 5.1
[CmdletBinding(SupportsShouldProcess)]
param(
    [string]$Destination,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$Payload = @(
    'AGENTS.local.md'
    'CLAUDE.local.md'
    'devkit.local.toml'
    'install.local.ps1'
    'setup.local.ps1'
)

function Get-MainCheckout {
    # `git worktree list --porcelain` names the main checkout first, whichever
    # worktree it is run from.
    $first = git -C $PSScriptRoot worktree list --porcelain | Select-Object -First 1
    if ($first -notmatch '^worktree (.+)$') {
        throw 'could not read the main checkout from git worktree list'
    }
    return $Matches[1]
}

if (-not $Destination) { $Destination = Get-MainCheckout }
$Destination = (Resolve-Path -LiteralPath $Destination).Path
$Source = (Resolve-Path -LiteralPath $PSScriptRoot).Path

if ($Source -eq $Destination) {
    throw "source and destination are the same checkout ($Source). Run this from the docs/specs-and-plans worktree."
}

Write-Host "syncing into $Destination"

foreach ($name in $Payload) {
    $from = Join-Path $Source $name
    if (-not (Test-Path -LiteralPath $from)) {
        Write-Warning "$name is not on this branch, skipping"
        continue
    }

    $to = Join-Path $Destination $name
    if (Test-Path -LiteralPath $to) {
        $same = (Get-FileHash -LiteralPath $from).Hash -eq (Get-FileHash -LiteralPath $to).Hash
        if ($same) {
            Write-Host "  unchanged  $name"
            continue
        }
        if (-not $Force) {
            Write-Warning "  differs    $name (pass -Force to overwrite)"
            continue
        }
    }

    if ($PSCmdlet.ShouldProcess($to, 'Copy')) {
        Copy-Item -LiteralPath $from -Destination $to -Force
        Write-Host "  copied     $name"
    }
}
