<#
.SYNOPSIS
Shows the logger's state: tasks, running programs, each capture's health,
the journal and index, disk space and the end of a log. Needs no elevation.

.DESCRIPTION
With -Watch it stays open and redraws every -Interval seconds. Keys:

  1 2 3   show the capture, index or compact log
  r       redraw now
  e       open the data folder in Explorer
  n       open the shown log in Notepad
  q       quit (also Esc or Ctrl-C)

  powershell -ExecutionPolicy Bypass -File deploy\windows\status.ps1 -Watch
#>
param(
  # Found from the installed tasks if not given.
  [string]$Root,
  [string]$Data,
  [switch]$Watch,
  [double]$Interval = 2,
  # Log lines shown without -Watch (with it, as many as fit).
  [int]$Lines = 5
)

$ErrorActionPreference = "Stop"
. "$PSScriptRoot\common.ps1"

$install = Get-Install
if (-not $Root) {
  $Root = if ($install) { $install.Root } else { Get-DefaultRoot User }
}
if (-not $Data) {
  $Data = if ($install) { $install.Data } else { "$Root\data" }
}
$logs = if ($install) { $install.Logs } else { "$Root\logs" }
$journal = "$Data\journal"

# A capture's status counts as stale after this long without a heartbeat,
# as capture itself judges it.
$staleMs = 5000

function Get-NowMs { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }

function Format-Age([long]$ms) {
  $s = [math]::Floor($ms / 1000)
  if ($s -lt 60) { return "${s}s" }
  $m = [math]::Floor($s / 60)
  if ($m -lt 60) { return "${m}m $($s % 60)s" }
  $h = [math]::Floor($m / 60)
  if ($h -lt 48) { return "${h}h $($m % 60)m" }
  "$([math]::Floor($h / 24))d $($h % 24)h"
}

function Format-Size([double]$bytes) {
  foreach ($unit in "B", "KB", "MB", "GB") {
    if ($bytes -lt 1024 -or $unit -eq "GB") {
      return "{0:0.#} {1}" -f $bytes, $unit
    }
    $bytes /= 1024
  }
}

function Get-WriteAge($item, $now) {
  $now - ([DateTimeOffset]$item.LastWriteTimeUtc).ToUnixTimeMilliseconds()
}

# The files in day directories under `$dir` (live\ or backfill\live\).
function Get-DayFiles($dir) {
  Get-ChildItem -LiteralPath $dir -Directory -ErrorAction SilentlyContinue |
    ForEach-Object {
      Get-ChildItem -LiteralPath $_.FullName -Filter *.journal -File
    }
}

function Get-Archives($dir) {
  @(Get-ChildItem -LiteralPath $dir -Recurse -Filter *.journal.zst -File `
    -ErrorAction SilentlyContinue)
}

# For the rate shown with -Watch: today's live bytes at the last redraw.
$script:previous = $null

# The status as lines of text, `$logLines` of them from `$log`.
function Get-Report($log, [int]$logLines) {
  $now = Get-NowMs
  $out = [Collections.Generic.List[string]]::new()
  $scope = if ($install) {
    "$($install.Scope.ToLower()) install"
    if (-not $install.Tasks) { " (shell runs, no tasks)" }
  } else { "not installed" }
  $out.Add("dgg-tools logger: $(-join $scope)   " +
    (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
  $drive = Get-PSDrive -Name $Data.Substring(0, 1) -ErrorAction SilentlyContinue
  $free = if ($drive) { "$(Format-Size $drive.Free) free" } else { "" }
  $out.Add("data $Data   $free")
  $out.Add("")

  $tasks = @(Get-ScheduledTask -TaskPath $TaskPath -ErrorAction SilentlyContinue)
  if ($tasks) {
    $out.Add("TASKS")
    foreach ($task in $tasks) {
      $info = $task | Get-ScheduledTaskInfo
      $result = "0x{0:x}" -f $info.LastTaskResult
      $next = if ($info.NextRunTime) { "next $($info.NextRunTime)" } else { "" }
      $out.Add(("  {0,-8} {1,-8} last result {2,-10} {3}" -f $task.TaskName,
        $task.State, $result, $next))
    }
    $out.Add("")
  }

  $out.Add("PROGRAMS")
  # A SYSTEM process's path is hidden from a non-administrator; it's shown
  # as running anyway, since only the install runs these names as SYSTEM.
  $programs = @(Get-Process capture, index, compact, backfill, sync, serve `
    -ErrorAction SilentlyContinue |
    Where-Object { -not $_.Path -or $_.Path -like "$Root\bin\*" })
  if (-not $programs) { $out.Add("  none running") }
  foreach ($program in $programs) {
    $up = if ($program.StartTime) {
      Format-Age ((Get-Date) - $program.StartTime).TotalMilliseconds
    } else { "?" }
    $build = if ($program.Path) { Split-Path (Split-Path $program.Path) -Leaf }
    else { "(path hidden)" }
    $out.Add(("  {0,-8} pid {1,-6} up {2,-9} {3,8} cpu {4,6:0}s  {5}" -f
      $program.Name, $program.Id, $up, (Format-Size $program.WorkingSet64),
      $program.CPU, $build))
  }
  $out.Add("")

  $out.Add("CAPTURES")
  $statuses = @(Get-ChildItem "$journal\control\*.status" `
    -ErrorAction SilentlyContinue)
  if (-not $statuses) { $out.Add("  none") }
  foreach ($file in $statuses) {
    $fields = @{}
    foreach ($line in Get-Content -LiteralPath $file.FullName) {
      $key, $value = $line -split " ", 2
      $fields[$key] = $value
    }
    $beat = $now - [long]$fields["heartbeat"]
    $state = if ($beat -gt $staleMs) { "STALE, last heard $(Format-Age $beat) ago" }
    elseif ($fields["healthy-since"]) {
      "healthy for $(Format-Age ($now - [long]$fields['healthy-since']))"
    } else { "connecting" }
    $out.Add(("  {0,-28} up {1,-9} {2}" -f $fields["instance"],
      (Format-Age ($now - [long]$fields["started"])), $state))
  }
  $out.Add("")

  $out.Add("JOURNAL")
  $today = [DateTime]::UtcNow.ToString("yyyy-MM-dd")
  $todays = @(Get-ChildItem "$journal\live\$today\*.journal" `
    -ErrorAction SilentlyContinue)
  $bytes = ($todays | Measure-Object Length -Sum).Sum
  $newest = $todays | Sort-Object LastWriteTimeUtc | Select-Object -Last 1
  $last = if ($newest) { "last write $(Format-Age (Get-WriteAge $newest $now)) ago" }
  else { "nothing yet" }
  $rate = ""
  if ($script:previous -and $script:previous.Day -eq $today) {
    $seconds = ($now - $script:previous.At) / 1000
    if ($seconds -gt 0) {
      $rate = "  +$(Format-Size (($bytes - $script:previous.Bytes) / $seconds))/s"
    }
  }
  $script:previous = @{ Day = $today; At = $now; Bytes = $bytes }
  $out.Add(("  today (UTC {0})  {1} files  {2}  {3}{4}" -f $today,
    $todays.Count, (Format-Size $bytes), $last, $rate))
  $older = @(Get-DayFiles "$journal\live" |
    Where-Object { $_.Directory.Name -ne $today })
  if ($older) {
    $days = @($older | ForEach-Object { $_.Directory.Name } | Sort-Object -Unique)
    $out.Add(("  waiting for compact  {0} days from {1}  {2}" -f $days.Count,
      $days[0], (Format-Size ($older | Measure-Object Length -Sum).Sum)))
  }
  $archives = Get-Archives "$journal\archive"
  if ($archives) {
    $names = @($archives | ForEach-Object { $_.Name.Substring(0, 10) } |
      Sort-Object)
    $out.Add(("  archived  {0} days, {1} to {2}  {3}" -f $archives.Count,
      $names[0], $names[-1], (Format-Size ($archives |
        Measure-Object Length -Sum).Sum)))
  }
  $backfill = @(Get-DayFiles "$journal\backfill\live")
  $backfillArchives = Get-Archives "$journal\backfill\archive"
  if ($backfill -or $backfillArchives) {
    $out.Add(("  backfill  {0} files waiting, {1} days archived  {2}" -f
      $backfill.Count, $backfillArchives.Count,
      (Format-Size ((@($backfill) + $backfillArchives |
        Measure-Object Length -Sum).Sum))))
  }
  $index = Get-Item -LiteralPath "$Data\index.sqlite" -ErrorAction SilentlyContinue
  if ($index) {
    $out.Add(("  index  {0}  last write {1} ago" -f (Format-Size $index.Length),
      (Format-Age (Get-WriteAge $index $now))))
  } else {
    $out.Add("  index  none yet")
  }
  $out.Add("")

  $path = "$logs\$log.log"
  $out.Add("LOG $path")
  if ($logLines -gt 0) {
    if (Test-Path -LiteralPath $path) {
      foreach ($line in Get-Content -LiteralPath $path -Tail $logLines) {
        $out.Add("  $line")
      }
    } else {
      $out.Add("  (none: shell runs print to their own windows)")
    }
  }
  $out
}

if (-not $Watch) {
  Get-Report "capture" $Lines
  foreach ($name in "index", "compact") {
    $path = "$logs\$name.log"
    if (Test-Path -LiteralPath $path) {
      "LOG $path"
      Get-Content -LiteralPath $path -Tail $Lines | ForEach-Object { "  $_" }
    }
  }
  return
}

if ([Console]::IsInputRedirected -or [Console]::IsOutputRedirected) {
  throw "-Watch needs an interactive console."
}
$log = "capture"
$keys = "[1] capture  [2] index  [3] compact log   [r] redraw  " +
  "[e] explorer  [n] notepad  [q] quit"
[Console]::CursorVisible = $false
[Console]::Clear()
try {
  while ($true) {
    $width = [Console]::WindowWidth - 1
    $height = [Console]::WindowHeight
    $report = @(Get-Report $log 0)
    $room = $height - $report.Count - 3
    if ($room -gt 0) {
      $path = "$logs\$log.log"
      $report += if (Test-Path -LiteralPath $path) {
        Get-Content -LiteralPath $path -Tail $room | ForEach-Object { "  $_" }
      } else { "  (none: shell runs print to their own windows)" }
    }
    $report = @($report | Select-Object -First ($height - 2)) + "" + $keys
    [Console]::SetCursorPosition(0, 0)
    foreach ($line in $report) {
      if ($line.Length -gt $width) { $line = $line.Substring(0, $width) }
      [Console]::WriteLine($line.PadRight($width))
    }
    # Blank whatever the last, longer redraw left below.
    for ($row = $report.Count; $row -lt $height - 1; $row++) {
      [Console]::WriteLine("".PadRight($width))
    }

    $until = (Get-Date).AddSeconds($Interval)
    $redraw = $false
    while (-not $redraw -and (Get-Date) -lt $until) {
      if (-not [Console]::KeyAvailable) {
        Start-Sleep -Milliseconds 100
        continue
      }
      $key = [Console]::ReadKey($true)
      switch ($key.KeyChar) {
        "1" { $log = "capture"; $redraw = $true }
        "2" { $log = "index"; $redraw = $true }
        "3" { $log = "compact"; $redraw = $true }
        "r" { $redraw = $true }
        "e" { Start-Process explorer.exe $Data }
        "n" {
          $path = "$logs\$log.log"
          if (Test-Path -LiteralPath $path) { Start-Process notepad.exe $path }
        }
        "q" { return }
      }
      if ($key.Key -eq "Escape") { return }
    }
  }
} finally {
  [Console]::CursorVisible = $true
  [Console]::Clear()
}
