<#
.SYNOPSIS
Installs, upgrades or moves the logger, as scheduled tasks.

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

Running it again with the same scope is an upgrade: it starts a second
capture from the new binaries, and the old one retires once the new one has
been healthy for the handoff grace (30 s). The index is stopped and
restarted, which is safe at any point. Data is never touched.

Running it with the other scope (or another -Root or -Data) moves the
install: the old tasks are removed, the data and logs are moved to the new
place (the data keeps its own place if it was outside the old root), the
old binaries are deleted, and the new tasks start. Capture stops for the
few seconds that takes; the history dgg sends on reconnect covers it. A
shell run's data in the user root (see README.md) is moved the same way.
Moving to or from a machine install needs an elevated PowerShell.

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
. "$PSScriptRoot\common.ps1"

$machine = $Scope -eq "Machine"
$admin = Test-Admin
$existing = Get-Install
if (-not $existing -and -not $admin -and
  (Test-Path -LiteralPath "$(Get-DefaultRoot Machine)\bin")) {
  throw "There seems to be a machine install, which only an " +
    "administrator can see. Run this from an elevated PowerShell."
}
$same = $existing -and $existing.Scope -eq $Scope
if (-not $Root) {
  $Root = if ($same) { $existing.Root } else { Get-DefaultRoot $Scope }
}
if (-not $Data) {
  $Data = if ($same) { $existing.Data }
  elseif ($existing -and -not (Test-Within $existing.Data $existing.Root)) {
    $existing.Data
  } else { "$Root\data" }
}
$logs = "$Root\logs"
$moving = $existing -and -not ($same -and
  (Test-Within $existing.Root $Root) -and (Test-Within $Root $existing.Root) -and
  (Test-Within $existing.Data $Data) -and (Test-Within $Data $existing.Data))
if (($machine -or ($existing -and $existing.Scope -eq "Machine")) -and
  -not $admin) {
  throw "Installing or moving a machine install needs an elevated " +
    "(administrator) PowerShell."
}
if ($moving -and -not (Test-Within $Data $existing.Data) -and
  -not (Test-Empty $Data)) {
  throw "$Data already has files, so the data at $($existing.Data) can't " +
    "move there. Move or delete them, then try again."
}

$repo = (Resolve-Path "$PSScriptRoot\..\..").Path

Write-Host "building the logger"
cargo build --release -p logger --manifest-path "$repo\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$commit = git -C $repo describe --always --dirty
$stamp = "$(Get-Date -Format yyyyMMdd-HHmmss)-$commit"
$bin = "$Root\bin\$stamp"

if ($machine) {
  # Before anything moves in: refuses a root someone else prepared.
  Protect-Directory $Root
}
if ($moving) {
  Write-Host ("moving the $($existing.Scope.ToLower()) install from " +
    "$($existing.Root)")
  Remove-Tasks $existing.Root
  try {
    Move-Directory $existing.Data $Data
    Move-Logs $existing.Logs $logs
  } catch {
    throw "Moving failed, and the old install's tasks are already " +
      "removed: $_`nFix the cause and run this again, or run it with " +
      "-Scope $($existing.Scope) to go back."
  }
  if (-not (Test-Within $existing.Root $Root)) { Remove-Root $existing.Root }
  if (-not $machine) {
    Unprotect-Directory $Data
    Unprotect-Directory $logs
  }
}
New-Item -ItemType Directory -Force $bin, $logs, $Data | Out-Null
if ($machine) {
  Protect-Directory $Root
  if (-not (Test-Within $Data $Root)) { Protect-Directory $Data -Adopt }
}
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
  $principal = New-ScheduledTaskPrincipal -UserId $User `
    -LogonType Interactive -RunLevel Limited
  $start = New-ScheduledTaskTrigger -AtLogOn -User $User
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

# Get-Install reads -Program, -Log and -Data back from these arguments.
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
Register-ScheduledTask -TaskPath $TaskPath -TaskName capture -Force `
  -Action (Supervised capture) -Trigger $start `
  -Principal $principal -Settings $settings | Out-Null

$settings = New-ScheduledTaskSettingsSet @always -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskPath $TaskPath -TaskName index -Force `
  -Action (Supervised index "-Follow") -Trigger $start `
  -Principal $principal -Settings $settings | Out-Null

$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable `
  -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -ExecutionTimeLimit (New-TimeSpan -Hours 4) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskPath $TaskPath -TaskName compact -Force `
  -Action (Supervised compact "-Once") `
  -Trigger (New-ScheduledTaskTrigger -Daily -At $CompactAt) `
  -Principal $principal -Settings $settings | Out-Null

# The old index (if any) is stopped outright; the old capture is left to
# hand off.
Stop-ScheduledTask -TaskPath $TaskPath -TaskName index
Stop-Running $Root index
Start-ScheduledTask -TaskPath $TaskPath -TaskName index
Start-ScheduledTask -TaskPath $TaskPath -TaskName capture
Write-Host "started capture and index; logs in $logs"

# Older builds go, unless something still runs from them.
Get-ChildItem "$Root\bin" -Directory | Sort-Object Name -Descending |
  Select-Object -Skip $Keep | ForEach-Object {
    Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
  }
