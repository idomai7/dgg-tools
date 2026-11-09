<#
.SYNOPSIS
Shows the logger's tasks, running processes, capture heartbeats and the end
of each log. Needs no elevation.
#>
param(
  [ValidateSet("Machine", "User")] [string]$Scope = "User",
  [string]$Root,
  [string]$Data,
  [int]$Lines = 5
)

if (-not $Root) {
  $Root = if ($Scope -eq "Machine") { "$env:ProgramData\dgg-tools" }
  else { "$env:LOCALAPPDATA\dgg-tools" }
}
if (-not $Data) { $Data = "$Root\data" }

Get-ScheduledTask -TaskPath "\dgg-tools\" -ErrorAction SilentlyContinue |
  ForEach-Object {
    $info = $_ | Get-ScheduledTaskInfo
    [pscustomobject]@{
      Task = $_.TaskName
      State = $_.State
      LastRun = $info.LastRunTime
      LastResult = $info.LastTaskResult
      NextRun = $info.NextRunTime
    }
  } | Format-Table -AutoSize

Get-Process capture, index, compact -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -like "$Root\bin\*" } |
  Select-Object Name, Id, StartTime, Path | Format-Table -AutoSize

Get-ChildItem "$Data\journal\control\*.status" -ErrorAction SilentlyContinue |
  ForEach-Object {
    Write-Host "== $($_.Name) (written $($_.LastWriteTime))"
    Get-Content $_.FullName
  }

Get-ChildItem "$Root\logs\*.log" -ErrorAction SilentlyContinue |
  ForEach-Object {
    Write-Host "== $($_.Name)"
    Get-Content $_.FullName -Tail $Lines
  }
