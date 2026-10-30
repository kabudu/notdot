# Record the official runner/toolset used for public Windows packages.
# Never export the SDK, headers, compiler, environment or credential material.
$ErrorActionPreference = 'Stop'
$destination = Join-Path $PWD 'target/windows-release-provenance'
New-Item -ItemType Directory -Force -Path $destination | Out-Null
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$installations = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json | ConvertFrom-Json)
if ($installations.Count -ne 1 -or $installations[0].installationVersion -notlike '17.*') {
  throw 'This release requires the official VS 2022 runner toolset.'
}
$installation = $installations[0]
$toolset = (Get-Content (Join-Path $installation.installationPath 'VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
$kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Lib'
$sdk = (Get-ChildItem $kits -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'ucrt/x64/libucrt.lib') } | Sort-Object Name -Descending | Select-Object -First 1).Name
if (-not $sdk) { throw 'Missing installed Windows SDK UCRT inventory.' }
$libraries = @(
  (Join-Path $installation.installationPath "VC/Tools/MSVC/$toolset/lib/x64/libcmt.lib"),
  (Join-Path $installation.installationPath "VC/Tools/MSVC/$toolset/lib/x64/libvcruntime.lib"),
  (Join-Path $kits "$sdk/ucrt/x64/libucrt.lib")
)
$inputs = @($libraries | ForEach-Object { @{ name = Split-Path $_ -Leaf; sha256 = (Get-FileHash $_ -Algorithm SHA256).Hash.ToLowerInvariant() } })
$compilerCaches = @(Get-ChildItem 'target/x86_64-pc-windows-msvc/release/build' -Recurse -File -Filter 'CMakeCache.txt' | Where-Object { $_.FullName -match 'whisper-rs-sys-' })
if (-not $compilerCaches.Count) { throw 'Missing actual native compiler build records.' }
$nativeCompilerRecords = @($compilerCaches | ForEach-Object {
  @{ cache = $_.FullName.Substring($PWD.Path.Length + 1); compiler = @(Select-String -Path $_.FullName -Pattern '^CMAKE_(C|CXX)_COMPILER:[^=]+=' | ForEach-Object { $_.Line }) }
})
$installers = @(Get-ChildItem 'target/x86_64-pc-windows-msvc/release/bundle/nsis' -File -Filter '*-setup.exe')
if ($installers.Count -ne 1) { throw 'Expected one actual NSIS release installer.' }
$record = @{
  revision = $env:GITHUB_SHA
  installer_sha256 = (Get-FileHash $installers[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  source = 'Official GitHub-hosted windows-2022 runner; no private xwin QA binaries'
  runnerImage = $env:ImageOS
  runnerVersion = $env:ImageVersion
  visualStudioVersion = $installation.installationVersion
  vcToolsetVersion = $toolset
  windowsSdkVersion = $sdk
  staticRuntimeInputs = $inputs
  nativeCompilerRecords = $nativeCompilerRecords
  terms = @('https://visualstudio.microsoft.com/license-terms/', 'https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution')
}
$record | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 (Join-Path $destination 'toolset.json')
# Retain the distributable list separately from notdot's Apache source licence.
Invoke-WebRequest -Uri 'https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution' -OutFile (Join-Path $destination 'Microsoft-VS2022-distributable-list.html') -TimeoutSec 60
$bundle = Join-Path $PWD 'target/x86_64-pc-windows-msvc/release/bundle'
$debugRuntime = @(Get-ChildItem $bundle -Recurse -File -Filter '*.dll' | Where-Object { $_.Name -match '^(msvcp\d+d|vcruntime\d+d|ucrtbased|concrtd\d*)\.dll$' })
if ($debugRuntime.Count) { throw 'Debug Microsoft runtimes must not be distributed.' }
Write-Output 'Windows runner/toolset provenance recorded without redistributing the toolchain.'
