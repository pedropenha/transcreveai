# Prepares the CURRENT PowerShell session for building Transcreve.ai on Windows.
# Dot-source it so the variables stick:   . .\scripts\windows-dev-env.ps1
#
# - VULKAN_SDK: picked from the newest C:\VulkanSDK\<version> when the installer's
#   value has not reached this terminal yet.
# - CARGO_TARGET_DIR: a short root (default D:\t — the build cache lives on D:,
#   keeping C: free) so the native Vulkan build of transcribe-cpp-sys stays
#   under MAX_PATH without its junction.
# - -BypassJunction: transcribe-cpp-sys builds through a junction under
#   %LOCALAPPDATA%\tcs. On machines where MSBuild refuses to open projects through
#   junctions (error MSB1009 "project file does not exist"), point LOCALAPPDATA at
#   a folder whose "tcs" entry is a file, so the junction cannot be created and the
#   build falls back to OUT_DIR. Only this session is affected.
#
# Nothing here is persisted; open a new terminal to undo.

[CmdletBinding()]
param(
    [string]$TargetDir = "D:\t",
    [switch]$BypassJunction
)

$ErrorActionPreference = "Stop"

if (-not $env:VULKAN_SDK) {
    $sdk = Get-ChildItem "C:\VulkanSDK" -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if ($sdk) {
        $env:VULKAN_SDK = $sdk.FullName
    } else {
        Write-Warning "Vulkan SDK not found. Install it: winget install KhronosGroup.VulkanSDK"
    }
}

$env:CARGO_TARGET_DIR = $TargetDir

if ($BypassJunction) {
    $shadow = Join-Path $TargetDir "localappdata-shadow"
    New-Item -ItemType Directory -Force $shadow | Out-Null
    $blocker = Join-Path $shadow "tcs"
    if (-not (Test-Path $blocker)) {
        New-Item -ItemType File $blocker | Out-Null
    }
    $env:TRANSCREVE_REAL_LOCALAPPDATA = $env:LOCALAPPDATA
    $env:LOCALAPPDATA = $shadow
}

Write-Host "VULKAN_SDK       = $env:VULKAN_SDK"
Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR"
if ($BypassJunction) {
    Write-Host "LOCALAPPDATA     = $env:LOCALAPPDATA (junction bypass; real: $env:TRANSCREVE_REAL_LOCALAPPDATA)"
}
