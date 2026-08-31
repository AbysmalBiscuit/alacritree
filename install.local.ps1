<#
.SYNOPSIS
Build the all-features alacritree in release mode and install it into ~/.local/bin.

.DESCRIPTION
Installs alacritree.exe together with the vendored console host it loads by
name.  A running alacritree pins its exe and conpty.dll, so an install that
cannot overwrite renames the pinned file aside and sweeps the leftovers on a
later run, once the process holding them has exited.
#>
#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$Branch = 'integration/all-features',
    [string]$Destination = (Join-Path $HOME '.local\bin'),
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'

# Same markers as alacritree/src/stale_exe.rs, so either side sweeps the other's
# leftovers and neither name is ever picked up by a PATH lookup.
$StaleMarker = '.stale-'
$TempMarker = '.tmp-'
$Payload = @('alacritree.exe', 'conpty.dll', 'OpenConsole.exe')

function Get-WorktreePath {
    param([string]$Branch)

    $path = $null
    foreach ($line in (git -C $PSScriptRoot worktree list --porcelain)) {
        if ($line -like 'worktree *') { $path = $line.Substring('worktree '.Length) }
        elseif ($line -eq "branch refs/heads/$Branch") { return $path }
    }
    throw "no worktree checked out on $Branch (git worktree add one first)"
}

function Test-FileLocked {
    param([string]$Path)

    try {
        $handle = [System.IO.File]::Open($Path, 'Open', 'Write', 'None')
        $handle.Close()
        return $false
    } catch [System.IO.IOException] {
        return $true
    } catch [System.UnauthorizedAccessException] {
        return $true
    }
}

function Remove-Leftovers {
    param([string]$Directory)

    $marker = "($([regex]::Escape($StaleMarker))|$([regex]::Escape($TempMarker)))"
    $names = ($Payload | ForEach-Object { [regex]::Escape($_) }) -join '|'
    $leftover = "^($names)$marker"

    foreach ($file in (Get-ChildItem -LiteralPath $Directory -File -ErrorAction SilentlyContinue)) {
        if ($file.Name -notmatch $leftover) { continue }
        # A leftover whose process is still running refuses deletion and waits
        # for a later sweep.
        Remove-Item -LiteralPath $file.FullName -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath $file.FullName)) { Write-Host "  swept $($file.Name)" }
    }
}

function Clear-InstalledName {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return }

    if (-not (Test-FileLocked $Path)) {
        Remove-Item -LiteralPath $Path -Force
        return
    }

    $leaf = Split-Path $Path -Leaf
    for ($attempt = 0; ; $attempt++) {
        $stale = "$leaf$StaleMarker$PID-$attempt"
        if (-not (Test-Path -LiteralPath (Join-Path (Split-Path $Path -Parent) $stale))) { break }
    }
    Rename-Item -LiteralPath $Path -NewName $stale
    Write-Host "  $leaf is in use, moved aside as $stale"
}

$worktree = Get-WorktreePath $Branch
$release = Join-Path $worktree 'target\release'

if (-not $SkipBuild) {
    Write-Host "building $Branch ($worktree)"
    ocargo.ps1 build -p alacritree --release --manifest-path (Join-Path $worktree 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}

$sources = $Payload |
    ForEach-Object { Join-Path $release $_ } |
    Where-Object { Test-Path -LiteralPath $_ }
if (-not ($sources | Where-Object { (Split-Path $_ -Leaf) -eq 'alacritree.exe' })) {
    throw "no alacritree.exe in $release"
}

if (-not (Test-Path -LiteralPath $Destination)) {
    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
}

Write-Host "installing into $Destination"
Remove-Leftovers $Destination

# Stage every file first: a copy that fails partway must not leave the
# destination without a working binary.
$staged = foreach ($source in $sources) {
    $leaf = Split-Path $source -Leaf
    $temp = Join-Path $Destination "$leaf$TempMarker$PID"
    Copy-Item -LiteralPath $source -Destination $temp -Force
    [pscustomobject]@{ Leaf = $leaf; Temp = $temp }
}

foreach ($file in $staged) {
    $target = Join-Path $Destination $file.Leaf
    Clear-InstalledName $target
    Move-Item -LiteralPath $file.Temp -Destination $target -Force
    Write-Host "  installed $($file.Leaf)"
}

Write-Host "done - $((Get-Item (Join-Path $Destination 'alacritree.exe')).LastWriteTime)"
