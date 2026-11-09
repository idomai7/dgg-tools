<#
.SYNOPSIS
Removes the logger's install, keeping only its data and logs.

.DESCRIPTION
Stops everything running from the install's binaries (shell runs from
<Root>\bin included), removes the scheduled tasks and their \dgg-tools\
folder, and deletes <Root>\bin. A machine install's data and logs get
ordinary permissions back, owned by this user, so they can be read, moved
or deleted without elevation. <Root> itself goes if nothing else is left in
it. The data directory (journal, archives, index) and logs\ are left in
place; delete them by hand to remove everything.

The install is found from its tasks. Without tasks, -Scope says which
default root to clean up. A machine install needs an elevated PowerShell.
#>
param(
  [ValidateSet("Machine", "User")] [string]$Scope,
  [string]$Root
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$install = Get-Install
if ($install -and -not $install.Tasks) { $install = $null }
if ($install -and $Scope -and $install.Scope -ne $Scope) {
  throw "The installed tasks are a $($install.Scope.ToLower()) install; " +
    "run this without -Scope, or with -Scope $($install.Scope)."
}
if (-not $Scope) { $Scope = if ($install) { $install.Scope } else { "User" } }
if (-not $Root) {
  $Root = if ($install) { $install.Root } else { Get-DefaultRoot $Scope }
}
$data = if ($install) { $install.Data } else { "$Root\data" }
$logs = if ($install) { $install.Logs } else { "$Root\logs" }
if ($Scope -eq "Machine" -and -not (Test-Admin)) {
  throw "Removing a machine install needs an elevated (administrator) " +
    "PowerShell."
}

Remove-Tasks $Root
Remove-Root $Root
if ($Scope -eq "Machine") {
  Unprotect-Directory $data
  Unprotect-Directory $logs
}
if ((Test-Path -LiteralPath $Root) -and (Test-Empty $Root)) {
  Remove-Item -LiteralPath $Root
}

Write-Host "removed the dgg-tools tasks and binaries"
foreach ($path in $data, $logs) {
  if (Test-Path -LiteralPath $path) { Write-Host "kept $path" }
}
