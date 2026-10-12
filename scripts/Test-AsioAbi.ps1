# Loads linktrix_audio_asio.dll through the unmodified Steinberg C++ IASIO
# interface (third_party\asio) and checks the ABI. Needs Visual Studio C++
# tools and the Windows SDK 10.0.26100.0. Build the DLL first:
#   cargo build --release; .\scripts\Test-AsioAbi.ps1
param([string]$Dll)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
if (-not $Dll) { $Dll=Join-Path $root 'target\release\linktrix_audio_asio.dll' }
$vswhere="${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vs=& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
$vc=(Get-ChildItem "$vs\VC\Tools\MSVC" -Directory | Sort-Object Name -Descending | Select-Object -First 1).FullName
$sdk="${env:ProgramFiles(x86)}\Windows Kits\10"
$out=Join-Path $root 'target\abi-test'
New-Item -ItemType Directory -Force $out | Out-Null
& "$vc\bin\Hostx64\x64\cl.exe" /nologo /EHsc /W4 "/I$root\third_party\asio" "/I$vc\include" "/I$sdk\Include\10.0.26100.0\ucrt" "/I$sdk\Include\10.0.26100.0\shared" "/I$sdk\Include\10.0.26100.0\um" "$root\tests\asio-abi.cpp" "/Fo$out\asio-abi.obj" "/Fe$out\asio-abi.exe" /link "/LIBPATH:$vc\lib\x64" "/LIBPATH:$sdk\Lib\10.0.26100.0\ucrt\x64" "/LIBPATH:$sdk\Lib\10.0.26100.0\um\x64" ole32.lib uuid.lib
if ($LASTEXITCODE) { throw 'C++ ASIO ABI harness build failed' }
& "$out\asio-abi.exe" $Dll
if ($LASTEXITCODE) { throw 'ASIO ABI mismatch' }
