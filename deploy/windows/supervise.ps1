<#
.SYNOPSIS
Runs one of the logger's binaries against a data directory, appending its
output to a log with a timestamp per line, and restarts it a minute after it
fails.

.DESCRIPTION
A clean exit (0) ends the loop: capture exits 0 when it hands off to a newer
instance, which must not be restarted. With -Once it never restarts.
#>
param(
  [Parameter(Mandatory)] [string]$Log,
  [Parameter(Mandatory)] [string]$Program,
  [Parameter(Mandatory)] [string]$Data,
  # Passes `--follow` (for index).
  [switch]$Follow,
  [switch]$Once
)

$ErrorActionPreference = "Stop"

$utf8 = [Text.UTF8Encoding]::new($false)

function Write-Log($line) {
  $time = Get-Date -Format "yyyy-MM-dd HH:mm:ss.fff"
  # Add-Content's UTF8 writes a byte-order mark.
  [IO.File]::AppendAllText($Log, "[$time] $line`n", $utf8)
}

$arguments = "--data `"$Data`""
if ($Follow) { $arguments += " --follow" }

while ($true) {
  Write-Log "starting $Program $arguments"
  $start = [Diagnostics.ProcessStartInfo]::new($Program, $arguments)
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  # The binaries only write to stderr; it's read here, line by line, so
  # the log stays current.
  $start.RedirectStandardError = $true
  $start.WorkingDirectory = Split-Path $Program
  $process = [Diagnostics.Process]::Start($start)
  while ($null -ne ($line = $process.StandardError.ReadLine())) {
    Write-Log $line
  }
  $process.WaitForExit()
  $code = $process.ExitCode
  Write-Log "exited with $code"
  if ($code -eq 0 -or $Once) { exit $code }
  Start-Sleep -Seconds 60
}
