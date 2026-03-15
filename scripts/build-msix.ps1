param(
  [Parameter(Mandatory = $true)]
  [string]$IdentityName,
  [Parameter(Mandatory = $true)]
  [string]$Publisher,
[Parameter(Mandatory = $false)]
  [string]$DisplayName = "assinador-livre-rs",
  [Parameter(Mandatory = $false)]
  [string]$PublisherDisplayName = "celsowm",
  [Parameter(Mandatory = $false)]
  [string]$Description = "Aplicacao desktop para assinatura digital de PDF com certificado A3 no Windows",
  [Parameter(Mandatory = $false)]
  [string]$Version,
  [Parameter(Mandatory = $false)]
  [ValidateSet("x64")]
  [string]$Architecture = "x64",
  [Parameter(Mandatory = $false)]
  [string]$TargetTriple = "x86_64-pc-windows-msvc",
  [Parameter(Mandatory = $false)]
  [string]$OutputDir = "target\\msix",
  [Parameter(Mandatory = $false)]
  [string]$LogoPath = "assets\\icone-assinador-livre.png",
  [Parameter(Mandatory = $false)]
  [string]$PfxPath,
  [Parameter(Mandatory = $false)]
  [string]$PfxPassword,
  [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-ToolPath {
  param(
    [string]$ToolName,
    [string[]]$Candidates
  )

  $found = Get-Command $ToolName -ErrorAction SilentlyContinue
  if ($found) {
    return $found.Source
  }

  foreach ($candidate in $Candidates) {
    if (Test-Path $candidate) {
      return $candidate
    }
  }

  throw "Ferramenta nao encontrada: $ToolName"
}

function Get-CargoVersion {
  $line = Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
  if (-not $line) {
    throw "Nao foi possivel ler a versao do Cargo.toml"
  }
  return $line.Matches[0].Groups[1].Value
}

function Convert-ToMsixVersion {
  param([string]$SemVer)
  $main = $SemVer.Split("-")[0]
  $parts = $main.Split(".")
  if ($parts.Count -lt 2) {
    throw "Versao invalida para MSIX: $SemVer"
  }
  while ($parts.Count -lt 4) {
    $parts += "0"
  }
  $nums = @()
  foreach ($p in $parts[0..3]) {
    [int]$n = 0
    if (-not [int]::TryParse($p, [ref]$n)) {
      throw "Versao invalida para MSIX: $SemVer"
    }
    if ($n -lt 0 -or $n -gt 65535) {
      throw "Componente de versao MSIX fora do limite (0..65535): $n"
    }
    $nums += $n
  }
  return ($nums -join ".")
}

$makeAppx = Get-ToolPath -ToolName "makeappx.exe" -Candidates @(
  "C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.26100.0\\x64\\makeappx.exe",
  "C:\\Program Files (x86)\\Windows Kits\\10\\App Certification Kit\\makeappx.exe"
)

$signTool = Get-ToolPath -ToolName "signtool.exe" -Candidates @(
  "C:\\Program Files (x86)\\Windows Kits\\10\\bin\\10.0.26100.0\\x64\\signtool.exe",
  "C:\\Program Files (x86)\\Windows Kits\\10\\App Certification Kit\\signtool.exe"
)

$appVersion = if ([string]::IsNullOrWhiteSpace($Version)) { Get-CargoVersion } else { $Version.Trim() }
$msixVersion = Convert-ToMsixVersion -SemVer $appVersion

if (-not (Test-Path $LogoPath)) {
  throw "Logo nao encontrado: $LogoPath"
}

if (-not $SkipBuild) {
  Write-Host "Building release binary..."
  cargo build --release --target $TargetTriple
}

$exePath = Join-Path (Get-Location) ("target\\$TargetTriple\\release\\assinador-livre-rs.exe")
if (-not (Test-Path $exePath)) {
  throw "Executavel nao encontrado: $exePath"
}

$outRoot = Join-Path (Get-Location) $OutputDir
$layout = Join-Path $outRoot "layout"
$assets = Join-Path $layout "Assets"
$appDir = Join-Path $layout "VFS\\ProgramFilesX64\\Assinador Livre\\bin"

if (Test-Path $layout) {
  Remove-Item -Recurse -Force $layout
}
New-Item -ItemType Directory -Force -Path $assets | Out-Null
New-Item -ItemType Directory -Force -Path $appDir | Out-Null

Copy-Item $exePath (Join-Path $appDir "assinador-livre-rs.exe") -Force
Copy-Item $LogoPath (Join-Path $assets "StoreLogo.png") -Force

$manifestPath = Join-Path $layout "AppxManifest.xml"
$manifest = @"
<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap desktop rescap">
  <Identity Name="$IdentityName" Publisher="$Publisher" Version="$msixVersion" ProcessorArchitecture="$Architecture" />
  <Properties>
    <DisplayName>$DisplayName</DisplayName>
    <PublisherDisplayName>$PublisherDisplayName</PublisherDisplayName>
    <Description>$Description</Description>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Resources>
    <Resource Language="en-us" />
    <Resource Language="pt-br" />
  </Resources>
  <Applications>
    <Application Id="AssinadorLivreRs" Executable="VFS\ProgramFilesX64\Assinador Livre\bin\assinador-livre-rs.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements
        DisplayName="$DisplayName"
        Description="$Description"
        BackgroundColor="transparent"
        Square150x150Logo="Assets\StoreLogo.png"
        Square44x44Logo="Assets\StoreLogo.png">
        <uap:DefaultTile Wide310x150Logo="Assets\StoreLogo.png" Square310x310Logo="Assets\StoreLogo.png" />
      </uap:VisualElements>
      <Extensions>
        <desktop:Extension Category="windows.fullTrustProcess" Executable="VFS\ProgramFilesX64\Assinador Livre\bin\assinador-livre-rs.exe" />
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>
</Package>
"@

$manifest | Out-File -FilePath $manifestPath -Encoding utf8

if (-not (Test-Path $outRoot)) {
  New-Item -ItemType Directory -Force -Path $outRoot | Out-Null
}

$msixFile = Join-Path $outRoot ("assinador-livre-rs-$appVersion-$Architecture.msix")
if (Test-Path $msixFile) {
  Remove-Item -Force $msixFile
}

Write-Host "Packing MSIX..."
& $makeAppx pack /d $layout /p $msixFile /o
if ($LASTEXITCODE -ne 0) {
  throw "makeappx falhou com codigo $LASTEXITCODE"
}

if (-not [string]::IsNullOrWhiteSpace($PfxPath)) {
  if (-not (Test-Path $PfxPath)) {
    throw "PFX nao encontrado: $PfxPath"
  }
  if ([string]::IsNullOrWhiteSpace($PfxPassword)) {
    throw "PfxPassword e obrigatorio quando PfxPath for usado."
  }

  Write-Host "Signing MSIX..."
  & $signTool sign /fd SHA256 /f $PfxPath /p $PfxPassword /tr "http://timestamp.digicert.com" /td SHA256 $msixFile
  if ($LASTEXITCODE -ne 0) {
    throw "signtool falhou com codigo $LASTEXITCODE"
  }
}

Write-Host ""
Write-Host "MSIX gerado: $msixFile"
Write-Host "Manifesto:  $manifestPath"
