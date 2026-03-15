param(
  [Parameter(Mandatory = $true)]
  [string]$IdentityName,
  [Parameter(Mandatory = $true)]
  [string]$Publisher,
  [Parameter(Mandatory = $false)]
  [string]$DisplayName = "Assinador Livre RS",
  [Parameter(Mandatory = $false)]
  [string]$PublisherDisplayName = "Assinador Livre",
  [Parameter(Mandatory = $false)]
  [string]$Description = "Aplicacao desktop para assinatura digital de PDF com certificado A3 no Windows",
  [Parameter(Mandatory = $false)]
  [string]$Version,
  [Parameter(Mandatory = $false)]
  [string]$PfxPath,
  [Parameter(Mandatory = $false)]
  [string]$PfxPassword
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repo = "celsowm/assinador-livre-rs"
$buildScript = Join-Path $PSScriptRoot "build-msix.ps1"

if (-not (Test-Path $buildScript)) {
  throw "Script nao encontrado: $buildScript"
}

$buildParams = @{
  IdentityName         = $IdentityName
  Publisher            = $Publisher
  DisplayName          = $DisplayName
  PublisherDisplayName = $PublisherDisplayName
  Description          = $Description
}

if (-not [string]::IsNullOrWhiteSpace($Version)) {
  $buildParams.Version = $Version
}
if (-not [string]::IsNullOrWhiteSpace($PfxPath)) {
  $buildParams.PfxPath = $PfxPath
  $buildParams.PfxPassword = $PfxPassword
}

& $buildScript @buildParams
if ($LASTEXITCODE -ne 0) {
  throw "Falha ao gerar MSIX."
}

$resolvedVersion = if ([string]::IsNullOrWhiteSpace($Version)) {
  (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
} else {
  $Version
}

$tag = "v$resolvedVersion"
$msixPath = Join-Path (Get-Location) ("target\\msix\\assinador-livre-rs-$resolvedVersion-x64.msix")
if (-not (Test-Path $msixPath)) {
  throw "MSIX nao encontrado: $msixPath"
}

Write-Host "Publicando MSIX no GitHub Release $tag..."
$null = & "C:\\Program Files\\GitHub CLI\\gh.exe" release view $tag --repo $repo 2>$null
if ($LASTEXITCODE -ne 0) {
  Write-Host "Release $tag nao existe. Criando..."
  & "C:\\Program Files\\GitHub CLI\\gh.exe" release create $tag --repo $repo --title $tag --notes "Release automatizada de MSIX."
  if ($LASTEXITCODE -ne 0) {
    throw "Falha ao criar release $tag."
  }
}

& "C:\\Program Files\\GitHub CLI\\gh.exe" release upload $tag $msixPath --repo $repo --clobber
if ($LASTEXITCODE -ne 0) {
  throw "Falha no upload do MSIX para release."
}

$assetUrl = "https://github.com/celsowm/assinador-livre-rs/releases/download/$tag/assinador-livre-rs-$resolvedVersion-x64.msix"
Write-Host "MSIX publicado: $assetUrl"
