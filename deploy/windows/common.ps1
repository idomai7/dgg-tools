# Shared by install.ps1, uninstall.ps1 and status.ps1: dot-source it.

$TaskPath = "\dgg-tools\"
$System = [Security.Principal.SecurityIdentifier]"S-1-5-18"
$Administrators = [Security.Principal.SecurityIdentifier]"S-1-5-32-544"
$User = "$env:USERDOMAIN\$env:USERNAME"

function Test-Admin {
  [Security.Principal.WindowsPrincipal]::new(
    [Security.Principal.WindowsIdentity]::GetCurrent()
  ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Get-DefaultRoot($scope) {
  if ($scope -eq "Machine") { "$env:ProgramData\dgg-tools" }
  else { "$env:LOCALAPPDATA\dgg-tools" }
}

# Whether `$path` is `$directory` or inside it.
function Test-Within($path, $directory) {
  $path = [IO.Path]::GetFullPath($path).TrimEnd("\")
  $directory = [IO.Path]::GetFullPath($directory).TrimEnd("\")
  $path -eq $directory -or
    $path.StartsWith("$directory\", "OrdinalIgnoreCase")
}

# What's installed, read back from the capture task: its scope, root, data
# and logs directories. Without tasks, a shell run's data in the user root
# counts as a user install with no tasks. $null if there's neither.
function Get-Install {
  $task = Get-ScheduledTask -TaskPath $TaskPath -TaskName capture `
    -ErrorAction SilentlyContinue
  if ($task) {
    $arguments = $task.Actions[0].Arguments
    $field = {
      param($name)
      if ($arguments -match "-$name `"([^`"]+)`"") { $Matches[1] }
    }
    $scope = if ($task.Principal.UserId -eq "SYSTEM") { "Machine" }
    else { "User" }
    return [pscustomobject]@{
      Scope = $scope
      # <root>\bin\<build>\capture.exe
      Root = Split-Path (Split-Path (Split-Path (& $field Program)))
      Data = & $field Data
      Logs = Split-Path (& $field Log)
      Tasks = $true
    }
  }
  $root = Get-DefaultRoot User
  if (Test-Path -LiteralPath "$root\data") {
    return [pscustomobject]@{
      Scope = "User"
      Root = $root
      Data = "$root\data"
      Logs = "$root\logs"
      Tasks = $false
    }
  }
  $null
}

# Stops everything running from `$root\bin` (the supervisors first, so
# nothing is restarted), and waits for it to exit. A kill loses nothing
# already written: the journal is written line by line, and its reader skips
# a half-written last line.
function Stop-Running($root, [string[]]$names = @("capture", "index",
  "compact")) {
  $bin = "$root\bin\*"
  $supervisors = @(Get-CimInstance Win32_Process -Filter `
    "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
    Where-Object {
      $line = $_.CommandLine
      $_.ProcessId -ne $PID -and $line -like "*$bin*supervise.ps1*" -and
        @($names | Where-Object { $line -like "*$bin\$_.exe*" })
    } |
    ForEach-Object { Get-Process -Id $_.ProcessId -ErrorAction Ignore })
  $programs = @(Get-Process $names -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -like $bin })
  foreach ($processes in $supervisors, $programs) {
    $processes | Stop-Process -Force -ErrorAction SilentlyContinue
    $processes | Wait-Process -Timeout 30 -ErrorAction SilentlyContinue
  }
}

# Stops and removes every task, and the task folder, then anything still
# running from `$root\bin`.
function Remove-Tasks($root) {
  Get-ScheduledTask -TaskPath $TaskPath -ErrorAction SilentlyContinue |
    ForEach-Object {
      Stop-ScheduledTask -TaskPath $TaskPath -TaskName $_.TaskName
      Unregister-ScheduledTask -TaskPath $TaskPath -TaskName $_.TaskName `
        -Confirm:$false
    }
  $service = New-Object -ComObject Schedule.Service
  $service.Connect()
  try {
    $service.GetFolder("\").DeleteFolder($TaskPath.Trim("\"), 0)
  } catch {
    # Already gone.
  }
  Stop-Running $root
}

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
# a junction sends SYSTEM's writes anywhere. -Adopt skips the owner check,
# for a directory this install just moved there itself.
function Protect-Directory($path, [switch]$Adopt) {
  if (Test-Path -LiteralPath $path) {
    $owner = (Get-Item -LiteralPath $path -Force).GetAccessControl(
      "Owner").GetOwner([Security.Principal.SecurityIdentifier])
    if (-not $Adopt -and $owner -ne $System -and
      $owner -ne $Administrators) {
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
  icacls $path /setowner "*S-1-5-32-544" /Q | Out-Null
  icacls $path /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" `
    "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX" /Q | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "icacls failed on $path" }
  if (Get-ChildItem -LiteralPath $path -Force) {
    # Drop every explicit entry and owner below, leaving only what's
    # inherited from `$path`.
    icacls "$path\*" /reset /T /C /Q | Out-Null
    icacls "$path\*" /setowner "*S-1-5-32-544" /T /C /Q | Out-Null
  }
}

# Undoes Protect-Directory: `$path` inherits from its parent again, and this
# user owns it.
function Unprotect-Directory($path) {
  if (-not (Test-Path -LiteralPath $path)) { return }
  icacls $path /reset /T /C /Q | Out-Null
  icacls $path /setowner $User /T /C /Q | Out-Null
}

# Whether `$path` is missing or empty.
function Test-Empty($path) {
  -not (Test-Path -LiteralPath $path) -or
    -not (Get-ChildItem -LiteralPath $path -Force)
}

# Moves the directory `$from` to `$to`, which must be missing or empty.
function Move-Directory($from, $to) {
  if ((Test-Path -LiteralPath $to) -and
    [IO.Path]::GetFullPath($from) -eq [IO.Path]::GetFullPath($to)) {
    return
  }
  if (-not (Test-Path -LiteralPath $from)) { return }
  if (-not (Test-Empty $to)) {
    throw "$to already has files; move or delete them, then try again."
  }
  if (Test-Path -LiteralPath $to) { Remove-Item -LiteralPath $to }
  New-Item -ItemType Directory -Force (Split-Path $to) | Out-Null
  Move-Item -LiteralPath $from -Destination $to
}

# Moves the logs in `$from` into `$to`, in front of any of the same name
# there (which, having been left behind by an earlier move, are newer).
function Move-Logs($from, $to) {
  if (-not (Test-Path -LiteralPath $from)) { return }
  New-Item -ItemType Directory -Force $to | Out-Null
  if ([IO.Path]::GetFullPath($from) -eq [IO.Path]::GetFullPath($to)) {
    return
  }
  Get-ChildItem -LiteralPath $from -Filter *.log | ForEach-Object {
    $target = Join-Path $to $_.Name
    if (Test-Path -LiteralPath $target) {
      $merged = [IO.File]::ReadAllBytes($_.FullName) +
        [IO.File]::ReadAllBytes($target)
      [IO.File]::WriteAllBytes($target, [byte[]]$merged)
      Remove-Item -LiteralPath $_.FullName
    } else {
      Move-Item -LiteralPath $_.FullName -Destination $target
    }
  }
  if (Test-Empty $from) { Remove-Item -LiteralPath $from }
}

# Removes `$root\bin`, and `$root` itself if nothing else is left in it.
function Remove-Root($root) {
  if (Test-Path -LiteralPath "$root\bin") {
    Remove-Item -LiteralPath "$root\bin" -Recurse -Force
  }
  if ((Test-Path -LiteralPath $root) -and (Test-Empty $root)) {
    Remove-Item -LiteralPath $root
  }
}
