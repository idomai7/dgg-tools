<#
.SYNOPSIS
Installs or upgrades the logger as scheduled tasks.

.DESCRIPTION
Builds the logger, copies the binaries to a new versioned directory under
<Root>\bin (so a running binary is never overwritten, and `cargo build`
never fails on a locked exe), and registers three tasks under \dgg-tools\:

  capture  at startup (logon), restarted a minute after any failure
  index    at startup (logon), `--follow`, restarted the same way
  compact  daily

-Scope User (the default) needs no elevation: the tasks run as this user,
from logon until logoff. -Scope Machine runs them as SYSTEM from startup,
whether or not anyone is logged on, and needs an elevated PowerShell.
With -Scope Machine, <Root> and the data directory are made writable only
by SYSTEM and Administrators (everyone else can read them), since anyone
who could change them could run code as SYSTEM; running the tools that
write there by hand then takes an elevated shell. The install refuses a
<Root> someone else created, or one containing links.

Running it again is an upgrade: it starts a second capture from the new
binaries, and the old one retires once the new one has been healthy for the
handoff grace (30 s). The index is stopped and restarted, which is safe at
any point. Data is never touched.

  powershell -ExecutionPolicy Bypass -File deploy\windows\install.ps1
  powershell -ExecutionPolicy Bypass -File deploy\windows\install.ps1 `
    -Scope Machine
#>
param(
  [ValidateSet("Machine", "User")] [string]$Scope = "User",
  # Holds bin\, logs\ and, by default, the data directory.
  [string]$Root,
  # The data directory: the journal and the index.
  [string]$Data,
  # Local time of the daily compaction.
  [string]$CompactAt = "03:00",
  # Builds kept in bin\, the running ones included.
  [int]$Keep = 3
)

$ErrorActionPreference = "Stop"

$machine = $Scope -eq "Machine"
if (-not $Root) {
  $Root = if ($machine) { "$env:ProgramData\dgg-tools" }
  else { "$env:LOCALAPPDATA\dgg-tools" }
}
if (-not $Data) { $Data = "$Root\data" }
$user = "$env:USERDOMAIN\$env:USERNAME"

$admin = [Security.Principal.WindowsPrincipal]::new(
  [Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if ($machine -and -not $admin) {
  throw "-Scope Machine needs an elevated (administrator) PowerShell."
}

$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$taskPath = "\dgg-tools\"

Write-Host "building the logger"
cargo build --release -p logger --manifest-path "$repo\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$commit = git -C $repo describe --always --dirty
$stamp = "$(Get-Date -Format yyyyMMdd-HHmmss)-$commit"
$bin = "$Root\bin\$stamp"
$logs = "$Root\logs"

$system = [Security.Principal.SecurityIdentifier]"S-1-5-18"
$administrators = [Security.Principal.SecurityIdentifier]"S-1-5-32-544"

# Junctions and symlinks under `$path`, without following them.
function Find-Links($path) {
  foreach ($entry in [IO.Directory]::EnumerateFileSystemEntries($path)) {
    $attributes = [IO.File]::GetAttributes($entry)
    if ($attributes -band [IO.FileAttributes]::ReparsePoint) {
      $entry
    } elseif ($attributes -band [IO.FileAttributes]::Directory) {
      Find-Links $entry
    }
  }
}

# Makes `$path` writable only by SYSTEM and Administrators, readable by
# everyone else. Whatever SYSTEM runs or writes to must not be changeable by
# anyone less: a replaced binary runs as SYSTEM, and a directory swapped for
# a junction sends SYSTEM's writes anywhere.
function Protect-Directory($path) {
  if (Test-Path -LiteralPath $path) {
    $owner = (Get-Item -LiteralPath $path -Force).GetAccessControl(
      "Owner").GetOwner([Security.Principal.SecurityIdentifier])
    if ($owner -ne $system -and $owner -ne $administrators) {
      throw "$path exists and isn't owned by Administrators or SYSTEM, " +
        "so someone else may have prepared it. Check it, then delete or " +
        "take ownership of it, and install again."
    }
    $links = @(Find-Links $path)
    if ($links) {
      throw "$path contains links, which SYSTEM would follow: " +
        ($links -join ", ")
    }
  } else {
    New-Item -ItemType Directory $path | Out-Null
  }
  icacls $path /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" `
    "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX" /Q | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "icacls failed on $path" }
  if (Get-ChildItem -LiteralPath $path -Force) {
    # Earlier installs granted this user write access; drop every explicit
    # entry and owner below, leaving only what's inherited from `$path`.
    icacls "$path\*" /reset /T /C /Q | Out-Null
    icacls "$path\*" /setowner "*S-1-5-32-544" /T /C /Q | Out-Null
  }
}

if ($machine) {
  Protect-Directory $Root
  $inRoot = [IO.Path]::GetFullPath($Data).StartsWith(
    [IO.Path]::GetFullPath($Root) + "\", "OrdinalIgnoreCase")
  if (-not $inRoot) { Protect-Directory $Data }
}
New-Item -ItemType Directory -Force $bin, $logs, $Data | Out-Null
foreach ($name in "capture", "index", "compact", "serve", "sync",
  "backfill", "gaps") {
  Copy-Item "$repo\target\release\$name.exe" $bin
}
Copy-Item "$PSScriptRoot\supervise.ps1" $bin
Write-Host "installed to $bin"

if ($machine) {
  $principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" `
    -LogonType ServiceAccount -RunLevel Highest
  $start = New-ScheduledTaskTrigger -AtStartup
} else {
  $principal = New-ScheduledTaskPrincipal -UserId $user `
    -LogonType Interactive -RunLevel Limited
  $start = New-ScheduledTaskTrigger -AtLogOn -User $user
}

$always = @{
  ExecutionTimeLimit = [TimeSpan]::Zero
  AllowStartIfOnBatteries = $true
  DontStopIfGoingOnBatteries = $true
  DontStopOnIdleEnd = $true
  StartWhenAvailable = $true
  # Task Scheduler's default (7) is below normal.
  Priority = 4
}

function Supervised($name, [string]$flags) {
  # Hidden, though a user-scope task's console may still flash at start.
  New-ScheduledTaskAction -Execute "powershell.exe" -WorkingDirectory $bin `
    -Argument ("-NoProfile -NonInteractive -WindowStyle Hidden " +
      "-ExecutionPolicy Bypass -File `"$bin\supervise.ps1`" " +
      "-Log `"$logs\$name.log`" -Program `"$bin\$name.exe`" " +
      "-Data `"$Data`" $flags")
}

# Parallel, so an upgrade's new capture runs beside the old until the
# handoff.
$settings = New-ScheduledTaskSettingsSet @always -MultipleInstances Parallel
Register-ScheduledTask -TaskPath $taskPath -TaskName capture -Force `
  -Action (Supervised capture) -Trigger $start `
  -Principal $principal -Settings $settings | Out-Null

$settings = New-ScheduledTaskSettingsSet @always -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskPath $taskPath -TaskName index -Force `
  -Action (Supervised index "-Follow") -Trigger $start `
  -Principal $principal -Settings $settings | Out-Null

$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -ExecutionTimeLimit (New-TimeSpan -Hours 4) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskPath $taskPath -TaskName compact -Force `
  -Action (Supervised compact "-Once") `
  -Trigger (New-ScheduledTaskTrigger -Daily -At $CompactAt) `
  -Principal $principal -Settings $settings | Out-Null

# The old index (if any) is stopped outright; the old capture is left to
# hand off.
Stop-ScheduledTask -TaskPath $taskPath -TaskName index
Get-Process index -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -like "$Root\bin\*" } | Stop-Process -Force
Start-ScheduledTask -TaskPath $taskPath -TaskName index
Start-ScheduledTask -TaskPath $taskPath -TaskName capture
Write-Host "started capture and index; logs in $logs"

# Older builds go, unless something still runs from them.
Get-ChildItem "$Root\bin" -Directory | Sort-Object Name -Descending |
  Select-Object -Skip $Keep | ForEach-Object {
    Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
  }
