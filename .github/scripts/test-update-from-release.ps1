# Updates an installed copy of the last published release with the installer this build made, the
# way the app's own updater runs it, and checks what the installer's hooks promise
# (apps/desktop/src-tauri/windows/hooks.nsh): the new program is in place with its shortcut and its
# entry in the list of apps; when the product's name changed since that release, the earlier
# program, shortcut and records are gone; and the data folders were not touched.
#
# Run by the Build workflow on Windows x64. Nothing here is a person's machine: the runner is
# thrown away afterwards, so the test installs for real.
param(
  [Parameter(Mandatory)] [string] $Installer,
  [Parameter(Mandatory)] [string] $Repository
)
$ErrorActionPreference = 'Stop'

# Release files are named <product>_<version>_<arch>-setup.exe.
function Get-ProductName([string] $file) {
  if ((Split-Path $file -Leaf) -match '^(?<name>.+?)_\d') { return $Matches.name }
  throw "Cannot tell the product from $file"
}

$config = Get-Content (Join-Path $PSScriptRoot '..\..\apps\desktop\src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$binary = "$($config.mainBinaryName).exe"
$work = Join-Path $env:RUNNER_TEMP 'update-from-release'
New-Item -ItemType Directory -Force $work | Out-Null
gh release download --repo $Repository --pattern '*_x64-setup.exe' --dir $work
if ($LASTEXITCODE -ne 0) { throw 'The last release has no Windows x64 installer to update from.' }
$old = Get-ChildItem $work -Filter '*_x64-setup.exe' | Select-Object -First 1
$new = Get-Item $Installer
$oldName = Get-ProductName $old.Name
$newName = Get-ProductName $new.Name
$menu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$apps = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
$oldFolder = Join-Path $env:LOCALAPPDATA $oldName
$newFolder = Join-Path $env:LOCALAPPDATA $newName

Write-Host "Installing the last release, $($old.Name)"
$run = Start-Process -FilePath $old.FullName -ArgumentList '/S' -Wait -PassThru
if ($run.ExitCode -ne 0) { throw "The last release's installer exited with $($run.ExitCode)." }
if (-not (Get-ChildItem $oldFolder -Filter '*.exe' -ErrorAction SilentlyContinue)) {
  throw "The last release did not install into $oldFolder."
}
if (-not (Test-Path (Join-Path $menu "$oldName.lnk"))) { throw "The last release made no Start menu shortcut." }

# Data as each earlier name kept it. No installer may touch it.
$data = @('codes.o7.desktop', 'si.auracode.desktop') | ForEach-Object {
  $folder = Join-Path $env:LOCALAPPDATA $_
  New-Item -ItemType Directory -Force (Join-Path $folder 'profiles\local') | Out-Null
  $file = Join-Path $folder 'profiles\local\kept.txt'
  Set-Content -Path $file -Value "kept in $_" -Encoding utf8
  $file
}

Write-Host "Updating with this build's installer, $($new.Name)"
$run = Start-Process -FilePath $new.FullName -ArgumentList '/S', '/UPDATE' -Wait -PassThru
if ($run.ExitCode -ne 0) { throw "This build's installer exited with $($run.ExitCode)." }

$problems = @()
function Expect([bool] $holds, [string] $what) {
  if ($holds) { Write-Host "ok: $what" } else { $script:problems += $what; Write-Host "NOT SO: $what" }
}
Expect (Test-Path (Join-Path $newFolder $binary)) "the program is at $newFolder\$binary"
Expect (Test-Path (Join-Path $menu "$newName.lnk")) "the Start menu has $newName"
Expect (Test-Path (Join-Path $apps $newName)) "the list of apps has $newName"
if ($oldName -ne $newName) {
  Expect (-not (Test-Path $oldFolder)) "the earlier program's folder, $oldFolder, is gone"
  Expect (-not (Test-Path (Join-Path $menu "$oldName.lnk"))) "the Start menu no longer has $oldName"
  Expect (-not (Test-Path (Join-Path $apps $oldName))) "the list of apps no longer has $oldName"
  Expect (-not (Test-Path "HKCU:\Software\$oldName\$oldName")) "the earlier installer's own records are gone"
}
foreach ($file in $data) { Expect (Test-Path $file) "the data is untouched: $file" }

# The installed program itself lists what belongs to it, the data kept under both earlier names
# included. `--dry-run` deletes nothing.
$listing = Join-Path $work 'cleanup.txt'
$run = Start-Process -FilePath (Join-Path $newFolder $binary) -ArgumentList '--uninstall-cleanup', '--dry-run' `
  -Wait -PassThru -RedirectStandardOutput $listing -RedirectStandardError (Join-Path $work 'cleanup-errors.txt')
$said = if (Test-Path $listing) { Get-Content $listing -Raw } else { '' }
Write-Host "The program's dry run of its cleanup exited with $($run.ExitCode) and said:`n$said"
if ($said) {
  Expect ($said -match 'codes\.o7\.desktop') 'the program names the data folder o7 used'
  Expect ($said -match 'si\.auracode\.desktop') 'the program names the data folder auracode used'
}
foreach ($file in $data) { Expect (Test-Path $file) "the data is still there after the dry run: $file" }

if ($problems.Count) { throw "The update did not do what it promises: $($problems -join '; ')" }
Write-Host 'The update did what it promises.'
