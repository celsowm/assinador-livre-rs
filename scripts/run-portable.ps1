param(
  [switch]$NoBuild,
  [switch]$Rebuild,
  [switch]$NoLaunch,
  [switch]$KeepRunning
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$portableDir = Join-Path $repoRoot "target\\windows-user-test"
$portableExe = Join-Path $portableDir "assinador-livre-rs.exe"
$portablePdfium = Join-Path $portableDir "pdfium.dll"

$releaseExe = Join-Path $repoRoot "target\\x86_64-pc-windows-msvc\\release\\assinador-livre-rs.exe"
$sourcePdfium = Join-Path $repoRoot "third_party\\pdfium\\windows-x64\\pdfium.dll"

function Get-LatestTrackedWriteTimeUtc([string]$RepoRoot) {
  $watchRoots = @(
    (Join-Path $RepoRoot "src"),
    (Join-Path $RepoRoot "ui"),
    (Join-Path $RepoRoot "assets")
  )

  $watchFiles = @(
    (Join-Path $RepoRoot "Cargo.toml"),
    (Join-Path $RepoRoot "Cargo.lock"),
    (Join-Path $RepoRoot "build.rs")
  )

  $times = New-Object System.Collections.Generic.List[datetime]

  foreach ($root in $watchRoots) {
    if (Test-Path $root) {
      Get-ChildItem -Path $root -File -Recurse | ForEach-Object {
        $times.Add($_.LastWriteTimeUtc)
      }
    }
  }

  foreach ($file in $watchFiles) {
    if (Test-Path $file) {
      $times.Add((Get-Item $file).LastWriteTimeUtc)
    }
  }

  if ($times.Count -eq 0) {
    return [datetime]::MinValue
  }

  return ($times | Measure-Object -Maximum).Maximum
}

function Stop-PortableProcessIfNeeded([switch]$KeepRunningFlag) {
  if ($KeepRunningFlag) {
    return
  }

  $running = Get-Process "assinador-livre-rs" -ErrorAction SilentlyContinue
  if ($null -ne $running) {
    Write-Host "Encerrando instancia em execucao para atualizar o portatil..."
    $running | Stop-Process -Force
  }
}

function Copy-WithRetry([string]$Source, [string]$Destination) {
  $maxAttempts = 20
  for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
    try {
      Copy-Item $Source $Destination -Force
      return
    } catch {
      if ($attempt -eq $maxAttempts) {
        throw
      }
      Start-Sleep -Milliseconds 250
    }
  }
}

if (-not (Test-Path $sourcePdfium)) {
  throw "PDFium nao encontrado: $sourcePdfium"
}

$needsBuild = $Rebuild.IsPresent
if (-not (Test-Path $releaseExe)) {
  $needsBuild = $true
} elseif (-not $Rebuild.IsPresent) {
  $latestSource = Get-LatestTrackedWriteTimeUtc -RepoRoot $repoRoot
  $releaseTime = (Get-Item $releaseExe).LastWriteTimeUtc
  if ($latestSource -gt $releaseTime) {
    $needsBuild = $true
  }
}

if ($needsBuild) {
  if ($NoBuild) {
    throw "Build necessario, mas -NoBuild foi informado. Execute sem -NoBuild ou rode cargo build --release."
  }

  if ($Rebuild) {
    Write-Host "Rebuild forcado solicitado. Executando build release..."
  } else {
    Write-Host "Release desatualizado/ausente. Executando build release..."
  }

  cargo build --release --target x86_64-pc-windows-msvc
  if ($LASTEXITCODE -ne 0) {
    throw "Falha no build release."
  }
}

Stop-PortableProcessIfNeeded -KeepRunningFlag:$KeepRunning

New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-WithRetry -Source $releaseExe -Destination $portableExe
Copy-WithRetry -Source $sourcePdfium -Destination $portablePdfium

Write-Host "Portatil pronto:"
Write-Host " - $portableExe"
Write-Host " - $portablePdfium"

if ($NoLaunch) {
  exit 0
}

Write-Host "Iniciando app portatil..."
Start-Process -FilePath $portableExe -WorkingDirectory $portableDir
