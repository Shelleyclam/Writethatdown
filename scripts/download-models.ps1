# Downloads the Whisper models used by whisperflow into .\models
#   base.en  (~148 MB)  fast, decent accuracy
#   small.en (~488 MB)  slower, noticeably more accurate
#
# Usage (from the project folder):
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 base.en

param([string[]]$Models = @("base.en", "small.en"))

$ErrorActionPreference = "Stop"
$dir = Join-Path $PSScriptRoot "..\models"
New-Item -ItemType Directory -Force -Path $dir | Out-Null

foreach ($m in $Models) {
    $file = Join-Path $dir "ggml-$m.bin"
    if (Test-Path $file) {
        Write-Host "ggml-$m.bin already downloaded"
        continue
    }
    $url = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-$m.bin"
    Write-Host "Downloading $m ..."
    # curl.exe ships with Windows 10/11 and is much faster than Invoke-WebRequest.
    curl.exe -L --fail --progress-bar -o "$file.part" $url
    if ($LASTEXITCODE -ne 0) { throw "Download of $m failed" }
    Move-Item "$file.part" $file
}
Write-Host "Done. Models are in $((Resolve-Path $dir).Path)"
