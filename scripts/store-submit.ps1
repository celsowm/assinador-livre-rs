param(
  [Parameter(Mandatory = $true)]
  [string]$TenantId,
  [Parameter(Mandatory = $true)]
  [string]$ClientId,
  [Parameter(Mandatory = $true)]
  [string]$ClientSecret,
  [Parameter(Mandatory = $true)]
  [string]$SellerId,
  [Parameter(Mandatory = $true)]
  [string]$ProductId,
  [Parameter(Mandatory = $true)]
  [string]$PackageUrl,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedVersion,
  [Parameter(Mandatory = $true)]
  [string]$ExpectedAssetName,
  [Parameter(Mandatory = $false)]
  [string[]]$Languages = @("pt-br", "en-us"),
  [Parameter(Mandatory = $false)]
  [string[]]$Architectures = @("X64"),
  [Parameter(Mandatory = $false)]
  [string]$InstallerParameters = "/qn /norestart",
  [switch]$Submit,
  [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$storeApiBase = "https://api.store.microsoft.com"
$tokenEndpoint = "https://login.microsoftonline.com/$TenantId/oauth2/v2.0/token"

function Set-GhaOutput {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,
    [Parameter(Mandatory = $true)]
    [string]$Value
  )

  if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_OUTPUT)) {
    "$Name=$Value" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
  }
}

function Get-StoreToken {
  param(
    [string]$Url,
    [string]$Client,
    [string]$Secret
  )

  $body = @{
    grant_type    = "client_credentials"
    client_id     = $Client
    client_secret = $Secret
    scope         = "https://api.store.microsoft.com/.default"
  }

  $response = Invoke-RestMethod -Method Post -Uri $Url -Body $body -ContentType "application/x-www-form-urlencoded"
  if ([string]::IsNullOrWhiteSpace($response.access_token)) {
    throw "Nao foi possivel obter access token para a Store API."
  }

  return $response.access_token
}

function Invoke-StoreApi {
  param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("GET", "POST", "PUT", "DELETE")]
    [string]$Method,
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [Parameter(Mandatory = $false)]
    [object]$Body,
    [Parameter(Mandatory = $true)]
    [string]$Token,
    [Parameter(Mandatory = $true)]
    [string]$Seller
  )

  $uri = $Path
  if (-not $uri.StartsWith("http", [System.StringComparison]::OrdinalIgnoreCase)) {
    $uri = "$storeApiBase$Path"
  }

  $headers = @{
    Authorization         = "Bearer $Token"
    "X-Seller-Account-Id" = $Seller
  }

  $params = @{
    Method      = $Method
    Uri         = $uri
    Headers     = $headers
    ContentType = "application/json"
  }

  if ($null -ne $Body) {
    $params.Body = ($Body | ConvertTo-Json -Depth 30)
  }

  try {
    return Invoke-RestMethod @params
  } catch {
    $statusCode = $null
    $statusDesc = $null
    $raw = $null

    if ($_.Exception.Response) {
      $statusCode = [int]$_.Exception.Response.StatusCode
      $statusDesc = $_.Exception.Response.StatusDescription
      try {
        $stream = $_.Exception.Response.GetResponseStream()
        if ($stream) {
          $reader = New-Object System.IO.StreamReader($stream)
          $raw = $reader.ReadToEnd()
        }
      } catch {
        $raw = $null
      }
    }

    $message = "Store API falhou em $Method $uri."
    if ($statusCode) {
      $message += " HTTP $statusCode $statusDesc."
    }
    if (-not [string]::IsNullOrWhiteSpace($raw)) {
      $message += " Resposta: $raw"
    }
    throw $message
  }
}

function Assert-StoreSuccess {
  param(
    [Parameter(Mandatory = $true)]
    [object]$Response,
    [Parameter(Mandatory = $true)]
    [string]$Operation
  )

  if ($Response.isSuccess -ne $true) {
    $msg = $null
    if ($Response.errors -and $Response.errors.Count -gt 0) {
      $msg = ($Response.errors | ConvertTo-Json -Depth 20 -Compress)
    }
    throw "Falha em $Operation. isSuccess=false. errors=$msg"
  }
}

function Get-OptionalProperty {
  param(
    [Parameter(Mandatory = $true)]
    [object]$Object,
    [Parameter(Mandatory = $true)]
    [string]$PropertyName
  )

  if ($null -eq $Object) {
    return $null
  }

  if ($Object.PSObject -and $Object.PSObject.Properties.Name -contains $PropertyName) {
    return $Object.$PropertyName
  }

  return $null
}

function Get-MsiProperty {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Path,
    [Parameter(Mandatory = $true)]
    [string]$PropertyName
  )

  $installer = New-Object -ComObject WindowsInstaller.Installer
  $database = $installer.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $installer, @($Path, 0))
  $query = "SELECT Value FROM Property WHERE Property='$PropertyName'"
  $view = $database.OpenView($query)
  $view.Execute()
  $record = $view.Fetch()
  if ($null -eq $record) {
    return $null
  }
  return $record.StringData(1)
}

Write-Host "Validando URL do pacote..."
$head = Invoke-WebRequest -Uri $PackageUrl -Method Head -MaximumRedirection 10
if ($head.StatusCode -lt 200 -or $head.StatusCode -ge 300) {
  throw "Package URL nao respondeu 2xx: $($head.StatusCode). URL: $PackageUrl"
}

if (-not $PackageUrl.Contains($ExpectedAssetName)) {
  throw "A URL do pacote nao contem o asset esperado '$ExpectedAssetName'. URL: $PackageUrl"
}

$tmpMsi = Join-Path $env:RUNNER_TEMP $ExpectedAssetName
Invoke-WebRequest -Uri $PackageUrl -OutFile $tmpMsi -MaximumRedirection 10

$downloadedVersion = Get-MsiProperty -Path $tmpMsi -PropertyName "ProductVersion"
$downloadedName = Get-MsiProperty -Path $tmpMsi -PropertyName "ProductName"
if ([string]::IsNullOrWhiteSpace($downloadedVersion)) {
  throw "Nao foi possivel ler ProductVersion do MSI baixado."
}

if ($downloadedVersion -ne $ExpectedVersion) {
  throw "Versao divergente no MSI. Esperado=$ExpectedVersion, encontrado=$downloadedVersion"
}

Write-Host "MSI validado: ProductName='$downloadedName' ProductVersion='$downloadedVersion'"

$sig = Get-AuthenticodeSignature -FilePath $tmpMsi
if ($sig.Status -ne "Valid") {
  throw "Assinatura do MSI invalida. Status=$($sig.Status)"
}

$packagePayload = @{
  packages = @(
    @{
      packageUrl          = $PackageUrl
      packageType         = "msi"
      architectures       = $Architectures
      languages           = $Languages
      isSilentInstall     = $true
      installerParameters = $InstallerParameters
    }
  )
}

$payloadPath = Join-Path $env:RUNNER_TEMP "store-package-payload-$ExpectedVersion.json"
$packagePayload | ConvertTo-Json -Depth 30 | Out-File -FilePath $payloadPath -Encoding utf8
Write-Host "Payload gerado em: $payloadPath"
Set-GhaOutput -Name "payload_path" -Value $payloadPath
Set-GhaOutput -Name "submission_id" -Value ""
Set-GhaOutput -Name "submission_status_url" -Value ""

if ($DryRun) {
  Write-Host "Dry-run ativo: nenhuma alteracao na Store sera enviada."
  return
}

Write-Host "Autenticando na Store API..."
$token = Get-StoreToken -Url $tokenEndpoint -Client $ClientId -Secret $ClientSecret

Write-Host "Checando status atual..."
$status = Invoke-StoreApi -Method GET -Path "/submission/v1/product/$ProductId/status" -Token $token -Seller $SellerId
Assert-StoreSuccess -Response $status -Operation "GET status"
$ongoingSubmissionId = Get-OptionalProperty -Object (Get-OptionalProperty -Object $status -PropertyName "responseData") -PropertyName "ongoingSubmissionId"
if (-not [string]::IsNullOrWhiteSpace($ongoingSubmissionId)) {
  throw "Ja existe submissao em andamento: $ongoingSubmissionId. Idempotencia bloqueou nova submissao."
}

Write-Host "Checando draft atual para idempotencia..."
$currentPackages = Invoke-StoreApi -Method GET -Path "/submission/v1/product/$ProductId/packages" -Token $token -Seller $SellerId
Assert-StoreSuccess -Response $currentPackages -Operation "GET packages"

$alreadyPresent = $false
$responseData = Get-OptionalProperty -Object $currentPackages -PropertyName "responseData"
$existingPackages = Get-OptionalProperty -Object $responseData -PropertyName "packages"
if ($existingPackages) {
  foreach ($pkg in $existingPackages) {
    $pkgUrl = Get-OptionalProperty -Object $pkg -PropertyName "packageUrl"
    if ($pkgUrl -eq $PackageUrl) {
      $alreadyPresent = $true
      break
    }
  }
}
if ($alreadyPresent) {
  throw "O draft atual ja contem este packageUrl. Idempotencia bloqueou nova execucao para a mesma versao."
}

Write-Host "Atualizando pacote da submissao..."
$putPackages = Invoke-StoreApi `
  -Method PUT `
  -Path "/submission/v1/product/$ProductId/packages" `
  -Body $packagePayload `
  -Token $token `
  -Seller $SellerId
Assert-StoreSuccess -Response $putPackages -Operation "PUT packages"

Write-Host "Iniciando commit de pacote..."
$commit = Invoke-StoreApi -Method POST -Path "/submission/v1/product/$ProductId/packages/commit" -Token $token -Seller $SellerId
Assert-StoreSuccess -Response $commit -Operation "POST packages/commit"

$packageCommitPollingUrl = Get-OptionalProperty -Object (Get-OptionalProperty -Object $commit -PropertyName "responseData") -PropertyName "pollingUrl"
if ([string]::IsNullOrWhiteSpace($packageCommitPollingUrl)) {
  throw "Commit retornou sem pollingUrl."
}
Write-Host "Polling commit em: $packageCommitPollingUrl"

$packageReady = $false
for ($attempt = 1; $attempt -le 40; $attempt++) {
  Start-Sleep -Seconds 15
  $poll = Invoke-StoreApi -Method GET -Path $packageCommitPollingUrl -Token $token -Seller $SellerId
  Assert-StoreSuccess -Response $poll -Operation "GET commit polling"

  $pollErrors = Get-OptionalProperty -Object $poll -PropertyName "errors"
  if ($pollErrors -and $pollErrors.Count -gt 0) {
    $errors = ($pollErrors | ConvertTo-Json -Depth 20 -Compress)
    throw "Erro no processamento de pacote: $errors"
  }

  $pollData = Get-OptionalProperty -Object $poll -PropertyName "responseData"
  $isReady = Get-OptionalProperty -Object $pollData -PropertyName "isReady"
  if ($isReady -eq $true) {
    $packageReady = $true
    break
  }

  Write-Host "Aguardando processamento do pacote... tentativa $attempt/40"
}

if (-not $packageReady) {
  throw "Timeout aguardando commit de pacote na Store."
}

if (-not $Submit) {
  Write-Host "Pacote commitado com sucesso. submit=false, submissao final nao sera enviada."
  return
}

Write-Host "Enviando submissao para certificacao..."
$submitResponse = Invoke-StoreApi -Method POST -Path "/submission/v1/product/$ProductId/submit" -Token $token -Seller $SellerId
Assert-StoreSuccess -Response $submitResponse -Operation "POST submit"

$submitData = Get-OptionalProperty -Object $submitResponse -PropertyName "responseData"
$submissionId = Get-OptionalProperty -Object $submitData -PropertyName "submissionId"
$submissionPollingUrl = Get-OptionalProperty -Object $submitData -PropertyName "pollingUrl"

if ([string]::IsNullOrWhiteSpace($submissionId)) {
  throw "Submit retornou sem submissionId."
}

Set-GhaOutput -Name "submission_id" -Value $submissionId
if (-not [string]::IsNullOrWhiteSpace($submissionPollingUrl)) {
  Set-GhaOutput -Name "submission_status_url" -Value $submissionPollingUrl
}

Write-Host "Submissao criada com sucesso. submissionId=$submissionId"
