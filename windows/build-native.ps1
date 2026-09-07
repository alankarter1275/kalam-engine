<#
.SYNOPSIS
    Build the C ABI as a Windows DLL for the .NET binding to load.

.DESCRIPTION
    `chapbook-ffi` already declares `cdylib` among its crate types — the
    same one Android loads — so this is `cargo build` and a report of where
    the file landed. It exists so that "how do I get the native half" has
    one answer in the repository rather than one per contributor, which is
    the same job `ios/build-xcframework.sh` does with rather more work.

    The DLL is not checked in. `Chapbook.csproj` copies it beside the
    assembly from `target/<profile>/`, and warns rather than fails when it
    is missing: the managed half compiles without it and every call throws
    `DllNotFoundException` at runtime, which is a clearer failure than a
    build error about a file nobody mentioned.

.PARAMETER Profile
    `release` (the default, and what a host ships) or `debug`, which is
    minutes faster and what to use while iterating on the Rust.

.PARAMETER Target
    A Rust target triple. Defaults to the host's. Pass
    `aarch64-pc-windows-msvc` for an Arm64 build; a package that means to
    carry both builds each and puts them under their own runtime
    identifier.

.EXAMPLE
    ./windows/build-native.ps1
    dotnet test windows/Chapbook.Tests/Chapbook.Tests.csproj

.EXAMPLE
    ./windows/build-native.ps1 -Profile debug
    dotnet test windows/Chapbook.Tests/Chapbook.Tests.csproj -p:ChapbookProfile=debug
#>
[CmdletBinding()]
param(
    [ValidateSet('release', 'debug')]
    [string]$Profile = 'release',
    [string]$Target = ''
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$cargoArgs = @('build', '-p', 'chapbook-ffi')
if ($Profile -eq 'release') { $cargoArgs += '--release' }
if ($Target) { $cargoArgs += @('--target', $Target) }

Write-Host "cargo $($cargoArgs -join ' ')"
Push-Location $root
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
finally {
    Pop-Location
}

$out = if ($Target) {
    Join-Path $root "target\$Target\$Profile"
} else {
    Join-Path $root "target\$Profile"
}
$dll = Join-Path $out 'chapbook_ffi.dll'
if (-not (Test-Path $dll)) {
    throw "cargo reported success but $dll is not there"
}

$size = [math]::Round((Get-Item $dll).Length / 1MB, 1)
Write-Host ""
Write-Host "chapbook_ffi.dll  $size MB  ->  $dll"
Write-Host ""
Write-Host "The managed projects pick it up from target\<profile>\. Build them with"
Write-Host "  dotnet build windows\Chapbook\Chapbook.csproj$(if ($Profile -eq 'debug') { ' -p:ChapbookProfile=debug' })"
