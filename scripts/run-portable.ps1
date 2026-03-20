param(
  [bool]$BuildIfMissing = $true,
  [switch]$NoLaunch
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$portableDir = Join-Path $repoRoot "target\\windows-user-test"
$portableExe = Join-Path $portableDir "assinador-livre-rs.exe"
$portablePdfium = Join-Path $portableDir "pdfium.dll"

$releaseExe = Join-Path $repoRoot "target\\x86_64-pc-windows-msvc\\release\\assinador-livre-rs.exe"
$sourcePdfium = Join-Path $repoRoot "third_party\\pdfium\\windows-x64\\pdfium.dll"

if (-not (Test-Path $sourcePdfium)) {
  throw "PDFium nao encontrado: $sourcePdfium"
}

if (-not (Test-Path $releaseExe)) {
  if ($BuildIfMissing) {
    Write-Host "Release nao encontrado. Executando build..."
    cargo build --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) {
      throw "Falha no build release."
    }
  } else {
    throw "Executavel release nao encontrado: $releaseExe"
  }
}

New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-Item $releaseExe $portableExe -Force
Copy-Item $sourcePdfium $portablePdfium -Force

Write-Host "Portatil pronto:"
Write-Host " - $portableExe"
Write-Host " - $portablePdfium"

if ($NoLaunch) {
  exit 0
}

Write-Host "Iniciando app portatil..."
Start-Process -FilePath $portableExe -WorkingDirectory $portableDir
