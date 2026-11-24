# Builds Familiar for Windows: the app (C# and WinUI 3, in windows/), the engine it loads
# (familiar_bridge.dll, Rust) and the terminal client. Run from anywhere, in PowerShell:
#
#   packaging\windows\build.ps1
#
# It needs Rust (any Windows toolchain; MSVC or MSYS2's) and the .NET 10 SDK. Visual Studio
# isn't needed. The result is dist\familiar\ (run Familiar.exe) and
# dist\familiar-<version>-x86_64-windows.zip. Everything the app needs is in the folder: .NET
# and the Windows App SDK are included, so there's nothing to install first.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$out = "dist\familiar"

cargo build --release --locked -p familiar-bridge -p familiar-tui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# The engine must stand alone: a DLL that needed MinGW's runtime would only work on the PC
# that built it.
$dll = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes((Resolve-Path "target\release\familiar_bridge.dll")))
foreach ($runtime in "libgcc_s_seh-1.dll", "libwinpthread-1.dll", "libstdc++-6.dll") {
    if ($dll.Contains($runtime)) { throw "familiar_bridge.dll depends on $runtime, which isn't part of Windows" }
}

# Emptied rather than removed: Windows won't delete a folder something has open.
New-Item -ItemType Directory -Force $out | Out-Null
Get-ChildItem $out -Force | Remove-Item -Recurse -Force

dotnet publish windows\Familiar\Familiar.csproj -c Release -r win-x64 --self-contained -p:Platform=x64 "-p:Version=$version" -o $out --nologo -v quiet
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed" }
Get-ChildItem $out -Filter *.pdb | Remove-Item

# The terminal client needs none of the rest.
Copy-Item target\release\familiar-tui.exe $out

New-Item -ItemType Directory -Force "$out\licenses" | Out-Null
Copy-Item LICENSE "$out\licenses\Familiar.txt"
Copy-Item THIRD-PARTY.md "$out\licenses\"

$zip = "dist\familiar-$version-x86_64-windows.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $out -DestinationPath $zip
"{0:N0} MB in {1}; {2:N0} MB zipped" -f ((Get-ChildItem $out -Recurse | Measure-Object Length -Sum).Sum / 1MB), $out, ((Get-Item $zip).Length / 1MB)
