# Builds psp\EBOOT.PBP inside the pspdev/pspdev Docker image (Windows PowerShell).
param([string[]]$MakeArgs = @())
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$image = if ($env:PSPDEV_IMAGE) { $env:PSPDEV_IMAGE } else { 'pspdev/pspdev:latest' }
docker run --rm -v "${root}\psp:/src" -w /src $image make @MakeArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
