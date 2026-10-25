@echo off
rem Creates AION.lnk next to this file. It opens aion.cmd as administrator, which ^ needs while the game runs
rem as administrator. The shortcut holds this PC's paths, so git ignores it.
set "AION_DIR=%~dp0."
powershell -NoProfile -Command ^
  "$d = (Resolve-Path $env:AION_DIR).Path; $q = [char]34; $lnk = Join-Path $d 'AION.lnk';" ^
  "Remove-Item -LiteralPath $lnk -ErrorAction Ignore; $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk); $s.WindowStyle = 1;" ^
  "$s.TargetPath = $env:ComSpec; $s.Arguments = '/c ' + $q + $q + (Join-Path $d 'aion.cmd') + $q + $q;" ^
  "$s.WorkingDirectory = $d; $s.IconLocation = (Join-Path $d 'assets\aion.ico') + ',0';" ^
  "$s.Description = 'AION: DPS meter, cube alert, kina tracker and hold buttons in one terminal'; $s.Save();" ^
  "$b = [IO.File]::ReadAllBytes($lnk); $b[0x15] = $b[0x15] -bor 0x20; [IO.File]::WriteAllBytes($lnk, $b);" ^
  "Write-Host ('created ' + $lnk)"
