<#
.SYNOPSIS
Stops and removes the logger's scheduled tasks. Keeps the binaries, logs and
data. -Scope Machine needs an elevated PowerShell.
#>
param(
  [ValidateSet("Machine", "User")] [string]$Scope = "User",
  [string]$Root
)

$ErrorActionPreference = "Stop"

if (-not $Root) {
  $Root = if ($Scope -eq "Machine") { "$env:ProgramData\dgg-tools" }
  else { "$env:LOCALAPPDATA\dgg-tools" }
}

Get-ScheduledTask -TaskPath "\dgg-tools\" -ErrorAction SilentlyContinue |
  ForEach-Object {
    Stop-ScheduledTask -TaskPath $_.TaskPath -TaskName $_.TaskName
    Unregister-ScheduledTask -TaskPath $_.TaskPath -TaskName $_.TaskName `
      -Confirm:$false
  }
# A kill is a crash as far as the journal is concerned, which it tolerates.
Get-Process capture, index, compact -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -like "$Root\bin\*" } | Stop-Process -Force
Write-Host "removed the dgg-tools tasks; $Root is left in place"
