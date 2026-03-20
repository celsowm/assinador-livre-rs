Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$manifestPath = Join-Path $PSScriptRoot "..\\third_party\\pdfium\\manifest.json"
if (-not (Test-Path $manifestPath)) {
  throw "Manifesto PDFium nao encontrado: $manifestPath"
}

$manifest = Get-Content -Raw $manifestPath | ConvertFrom-Json
if (-not $manifest.artifacts) {
  throw "Manifesto PDFium invalido: campo artifacts ausente."
}

$manifestDir = Split-Path -Parent $manifestPath

foreach ($artifact in $manifest.artifacts) {
  $file = Join-Path $manifestDir $artifact.file
  if (-not (Test-Path $file)) {
    throw "Arquivo PDFium ausente: $file"
  }

  $actual = (Get-FileHash $file -Algorithm SHA256).Hash.ToUpperInvariant()
  $expected = $artifact.sha256.ToString().ToUpperInvariant()
  if ($actual -ne $expected) {
    throw "Hash PDFium invalido para '$($artifact.file)'. Esperado: $expected, atual: $actual"
  }
}

Write-Host "PDFium hashes validados com sucesso."
