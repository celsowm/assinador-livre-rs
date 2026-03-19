use crate::{
    contracts::{
        CertSelectionRequest, CertificateSummary, VisibleSignaturePlacement,
        VisibleSignatureRequest, VisibleSignatureStyle, VisibleSignatureTimezone,
    },
    logger,
    runtime::{
        CertDialogInput, CertDialogOutput, DesktopRuntime, INSTANCE_MUTEX_NAME,
        SingleInstanceGuard, TrayCommand, TrayGuard, UiMessageLevel,
    },
};
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};
use single_instance::SingleInstance;
use std::{
    ffi::OsStr,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::Sender,
};
use tray_item::{IconSource, TrayItem};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LR_LOADFROMFILE, LoadIconW, LoadImageW,
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE_NAME: &str = "AssinadorLivre";
const ICON_FILE_NAME: &str = "icone-assinador-livre.ico";

pub struct WindowsRuntime;

impl WindowsRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl DesktopRuntime for WindowsRuntime {
    fn single_instance_guard(&self) -> Result<Box<dyn SingleInstanceGuard>> {
        let instance = SingleInstance::new(INSTANCE_MUTEX_NAME)
            .context("Falha ao criar mutex de instancia unica")?;
        if !instance.is_single() {
            bail!("Outra instancia do Assinador Livre ja esta em execucao.");
        }
        Ok(Box::new(instance))
    }

    fn create_tray(&self, command_tx: Sender<TrayCommand>) -> Result<Box<dyn TrayGuard>> {
        let icon = resolve_icon_path()
            .and_then(|path| {
                let loaded = load_icon_from_file(&path);
                if loaded.is_none() {
                    logger::warn(format!(
                        "Nao foi possivel carregar icone customizado da bandeja em {}. Usando padrao do Windows.",
                        path.display()
                    ));
                }
                loaded
            })
            .unwrap_or_else(|| unsafe { LoadIconW(0, IDI_APPLICATION) });

        if icon == 0 {
            bail!("Falha ao carregar icone padrao do Windows");
        }

        let mut tray = TrayItem::new("Assinador Livre", IconSource::RawIcon(icon))
            .context("Falha ao criar icone da bandeja")?;

        let sign_tx = command_tx.clone();
        tray.add_menu_item("Assinar documento", move || {
            let _ = sign_tx.send(TrayCommand::SignDocument);
        })
        .context("Falha ao criar item de menu 'Assinar documento'")?;

        let playground_tx = command_tx.clone();
        tray.add_menu_item("Abrir playground", move || {
            let _ = playground_tx.send(TrayCommand::OpenPlayground);
        })
        .context("Falha ao criar item de menu 'Abrir playground'")?;

        tray.add_menu_item("Sair", move || {
            let _ = command_tx.send(TrayCommand::Exit);
        })
        .context("Falha ao criar item de menu 'Sair'")?;

        Ok(Box::new(TrayHandle { _tray: tray }))
    }

    fn show_message(&self, level: UiMessageLevel, title: &str, description: &str) {
        let dialog_level = match level {
            UiMessageLevel::Warning => MessageLevel::Warning,
            UiMessageLevel::Error => MessageLevel::Error,
        };

        MessageDialog::new()
            .set_title(title)
            .set_description(description)
            .set_level(dialog_level)
            .set_buttons(MessageButtons::Ok)
            .show();
    }

    fn pick_pdfs(&self) -> Vec<PathBuf> {
        let desktop = std::env::var("USERPROFILE")
            .map(|h| PathBuf::from(h).join("Desktop"))
            .unwrap_or_else(|_| PathBuf::from("."));
        FileDialog::new()
            .set_title("Selecione os PDFs para assinar")
            .add_filter("Arquivos PDF", &["pdf"])
            .set_directory(&desktop)
            .pick_files()
            .unwrap_or_default()
    }

    fn choose_certificate_and_visible_signature(
        &self,
        input: CertDialogInput,
    ) -> Result<Option<CertDialogOutput>> {
        let selected = show_certificate_dropdown_dialog(
            &input.candidates,
            input.preselected_position,
            input.preview_pdf.as_deref(),
        )?;

        let Some(selected) = selected else {
            return Ok(None);
        };

        let cert_selection = if selected.use_auto {
            None
        } else {
            if selected.index >= input.candidates.len() {
                bail!(
                    "Indice selecionado fora da faixa: {} (max {})",
                    selected.index,
                    input.candidates.len().saturating_sub(1)
                );
            }
            let cert = &input.candidates[selected.index];
            Some(CertSelectionRequest {
                thumbprint: Some(cert.thumbprint.clone()),
                index: Some(cert.index),
            })
        };

        let visible_signature = if selected.visible_signature {
            let placement_raw = selected.placement.as_deref().unwrap_or_else(|| {
                if selected.manual_rect.is_some() {
                    "center_horizontal"
                } else {
                    "bottom_center_horizontal"
                }
            });
            let placement = parse_visible_signature_placement(placement_raw).ok_or_else(|| {
                anyhow::anyhow!(
                    "Posicao de assinatura visivel invalida retornada pelo dialogo: '{}'",
                    placement_raw
                )
            })?;

            Some(VisibleSignatureRequest {
                placement,
                custom_rect: selected.manual_rect,
                style: VisibleSignatureStyle::Default,
                timezone: VisibleSignatureTimezone::Local,
            })
        } else {
            None
        };

        Ok(Some(CertDialogOutput {
            cert_selection,
            visible_signature,
        }))
    }

    fn open_url(&self, url: &str) -> Result<()> {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("Falha ao abrir URL no navegador")?;
        Ok(())
    }

    fn set_startup(&self, enabled: bool) -> Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = hkcu
            .create_subkey(RUN_KEY_PATH)
            .context("Falha ao abrir chave Run")?;

        if enabled {
            let exe_path =
                std::env::current_exe().context("Falha ao descobrir caminho do executavel")?;
            let value = format!("\"{}\"", exe_path.display());
            run_key
                .set_value(RUN_VALUE_NAME, &value)
                .context("Falha ao escrever entrada de inicializacao")?;
        } else {
            let _ = run_key.delete_value(RUN_VALUE_NAME);
        }

        Ok(())
    }
}

struct TrayHandle {
    _tray: TrayItem,
}

fn resolve_icon_path() -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let exe_dir = exe_path.parent()?;
    let candidates = [
        exe_dir.join("assets").join(ICON_FILE_NAME),
        exe_dir
            .parent()
            .map(|p| p.join("assets").join(ICON_FILE_NAME))?,
        exe_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("assets").join(ICON_FILE_NAME))?,
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn load_icon_from_file(path: &Path) -> Option<HICON> {
    let wide_path: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let icon = unsafe {
        LoadImageW(
            0,
            wide_path.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTCOLOR | LR_LOADFROMFILE,
        )
    };

    if icon == 0 {
        return None;
    }

    Some(icon)
}

fn parse_visible_signature_placement(raw: &str) -> Option<VisibleSignaturePlacement> {
    match raw {
        "top_left_horizontal" => Some(VisibleSignaturePlacement::TopLeftHorizontal),
        "top_left_vertical" => Some(VisibleSignaturePlacement::TopLeftVertical),
        "top_right_horizontal" => Some(VisibleSignaturePlacement::TopRightHorizontal),
        "top_right_vertical" => Some(VisibleSignaturePlacement::TopRightVertical),
        "bottom_left_horizontal" => Some(VisibleSignaturePlacement::BottomLeftHorizontal),
        "bottom_left_vertical" => Some(VisibleSignaturePlacement::BottomLeftVertical),
        "bottom_right_horizontal" => Some(VisibleSignaturePlacement::BottomRightHorizontal),
        "bottom_right_vertical" => Some(VisibleSignaturePlacement::BottomRightVertical),
        "bottom_center_horizontal" => Some(VisibleSignaturePlacement::BottomCenterHorizontal),
        "bottom_center_vertical" => Some(VisibleSignaturePlacement::BottomCenterVertical),
        "center_horizontal" => Some(VisibleSignaturePlacement::CenterHorizontal),
        "center_vertical" => Some(VisibleSignaturePlacement::CenterVertical),
        _ => None,
    }
}

#[derive(Deserialize)]
struct TrayDropdownResult {
    use_auto: bool,
    index: usize,
    visible_signature: bool,
    placement: Option<String>,
    manual_rect: Option<[f32; 4]>,
}

#[derive(Serialize)]
struct TrayDropdownItem {
    display: String,
}

fn show_certificate_dropdown_dialog(
    candidates: &[CertificateSummary],
    preselected_position: usize,
    preview_pdf: Option<&Path>,
) -> Result<Option<TrayDropdownResult>> {
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    if candidates.is_empty() {
        return Ok(None);
    }

    let items: Vec<TrayDropdownItem> = candidates
        .iter()
        .map(|cert| {
            let token = if cert.is_hardware_token {
                "A3/token"
            } else {
                "software"
            };
            let provider = if cert.provider_name.is_empty() {
                "(sem provider)"
            } else {
                cert.provider_name.as_str()
            };
            let subject = if cert.subject.is_empty() {
                "(sem subject)"
            } else {
                cert.subject.as_str()
            };
            TrayDropdownItem {
                display: format!("[{}] {} | {} | {}", cert.index, subject, token, provider),
            }
        })
        .collect();

    let json = serde_json::to_string(&items)?;
    let payload_b64 = STANDARD.encode(json.as_bytes());
    let preview_path = preview_pdf
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let preview_b64 = STANDARD.encode(preview_path.as_bytes());
    let script_template = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'

$json = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PAYLOAD_B64__'))
$items = $json | ConvertFrom-Json
$previewPdfPath = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('__PREVIEW_B64__'))
$script:manualRect = $null

$form = New-Object System.Windows.Forms.Form
$form.Text = 'Selecao de certificado'
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.Width = 820
$form.Height = 360
$form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
$form.MaximizeBox = $false
$form.MinimizeBox = $false

$label = New-Object System.Windows.Forms.Label
$label.Left = 18
$label.Top = 20
$label.Width = 770
$label.Height = 36
$label.Text = 'Escolha o certificado para assinatura (pre-selecionado pelo algoritmo atual):'

$combo = New-Object System.Windows.Forms.ComboBox
$combo.Left = 18
$combo.Top = 62
$combo.Width = 770
$combo.DropDownStyle = [System.Windows.Forms.ComboBoxStyle]::DropDownList
[void]$combo.Items.Add('automatico (algoritmo atual)')
foreach ($item in $items) {{ [void]$combo.Items.Add($item.display) }}

$pre = __PRESELECTED_POSITION__
if ($combo.Items.Count -gt 1) {{
  $sel = $pre + 1
  if ($sel -lt 0 -or $sel -ge $combo.Items.Count) {{ $sel = 0 }}
  $combo.SelectedIndex = $sel
}} elseif ($combo.Items.Count -eq 1) {{
  $combo.SelectedIndex = 0
}}

$chkVisible = New-Object System.Windows.Forms.CheckBox
$chkVisible.Left = 18
$chkVisible.Top = 102
$chkVisible.Width = 250
$chkVisible.Text = 'Assinatura visivel'

$labelPos = New-Object System.Windows.Forms.Label
$labelPos.Left = 18
$labelPos.Top = 128
$labelPos.Width = 260
$labelPos.Height = 20
$labelPos.Text = 'Posicao da assinatura visivel:'

$comboPlacement = New-Object System.Windows.Forms.ComboBox
$comboPlacement.Left = 18
$comboPlacement.Top = 150
$comboPlacement.Width = 310
$comboPlacement.DropDownStyle = [System.Windows.Forms.ComboBoxStyle]::DropDownList
$positions = @(
  'top_left_horizontal',
  'top_left_vertical',
  'top_right_horizontal',
  'top_right_vertical',
  'bottom_left_horizontal',
  'bottom_left_vertical',
  'bottom_right_horizontal',
  'bottom_right_vertical',
  'bottom_center_horizontal',
  'bottom_center_vertical',
  'center_horizontal',
  'center_vertical'
)
foreach ($pos in $positions) {{ [void]$comboPlacement.Items.Add($pos) }}
$defaultPos = 'bottom_center_horizontal'
$defaultIdx = [array]::IndexOf($positions, $defaultPos)
if ($defaultIdx -lt 0) {{ $defaultIdx = 0 }}
if ($comboPlacement.Items.Count -gt 0) {{ $comboPlacement.SelectedIndex = $defaultIdx }}
$comboPlacement.Enabled = $false

$chkManual = New-Object System.Windows.Forms.CheckBox
$chkManual.Left = 350
$chkManual.Top = 152
$chkManual.Width = 330
$chkManual.Text = 'Posicionar manualmente na primeira pagina'
$chkManual.Enabled = $false

$btnManual = New-Object System.Windows.Forms.Button
$btnManual.Left = 350
$btnManual.Top = 182
$btnManual.Width = 160
$btnManual.Height = 28
$btnManual.Text = 'Abrir preview...'
$btnManual.Enabled = $false

$lblManualStatus = New-Object System.Windows.Forms.Label
$lblManualStatus.Left = 520
$lblManualStatus.Top = 188
$lblManualStatus.Width = 270
$lblManualStatus.Height = 34
$lblManualStatus.Text = 'Posicao manual nao definida.'

function Get-PresetSize([string]$placement) {{
  if ($placement -like '*_vertical') {{ return @(110.0, 180.0) }}
  return @(220.0, 72.0)
}}

function Clamp([double]$v, [double]$min, [double]$max) {{
  if ($v -lt $min) {{ return $min }}
  if ($v -gt $max) {{ return $max }}
  return $v
}}

function Show-ManualPlacementDialog([string]$pdfPath, [string]$placement) {{
  if ([string]::IsNullOrWhiteSpace($pdfPath) -or -not (Test-Path -LiteralPath $pdfPath)) {{
    [System.Windows.Forms.MessageBox]::Show('Nao foi possivel localizar o PDF para preview.', 'Preview indisponivel', [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Warning) | Out-Null
    return $null
  }}

  $bmp = $null
  $imgWidth = 900.0
  $imgHeight = 1200.0
  try {{
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
    $null = [Windows.Data.Pdf.PdfDocument, Windows.Data.Pdf, ContentType = WindowsRuntime]
    $null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
    $null = [System.IO.WindowsRuntimeStreamExtensions]

    $asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {{
      $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and $_.GetParameters().Count -eq 1
    }} | Select-Object -First 1)
    $asTaskAction = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {{
      $_.Name -eq 'AsTask' -and -not $_.IsGenericMethod -and $_.GetParameters().Count -eq 1
    }} | Select-Object -First 1)
    if (-not $asTaskGeneric -or -not $asTaskAction) {{ throw 'AsTask indisponivel.' }}

    $awaitTyped = {{
      param($op, [Type]$t)
      $task = $asTaskGeneric.MakeGenericMethod($t).Invoke($null, @($op))
      $task.Wait()
      if ($task.IsFaulted) {{ throw $task.Exception }}
      $task.Result
    }}
    $awaitAction = {{
      param($op)
      $task = $asTaskAction.Invoke($null, @($op))
      $task.Wait()
      if ($task.IsFaulted) {{ throw $task.Exception }}
    }}

    $storageFile = & $awaitTyped ([Windows.Storage.StorageFile]::GetFileFromPathAsync($pdfPath)) ([Windows.Storage.StorageFile])
    $pdfDoc = & $awaitTyped ([Windows.Data.Pdf.PdfDocument]::LoadFromFileAsync($storageFile)) ([Windows.Data.Pdf.PdfDocument])
    if ($pdfDoc.PageCount -lt 1) {{ throw 'PDF sem paginas.' }}
    $page = $pdfDoc.GetPage(0)
    $pageSize = $page.Size
    if ($pageSize.Width -gt 1 -and $pageSize.Height -gt 1) {{
      $scale = 900.0 / [double]$pageSize.Width
      $imgWidth = 900.0
      $imgHeight = [Math]::Round([double]$pageSize.Height * $scale)
      if ($imgHeight -lt 200) {{ $imgHeight = 200.0 }}
      if ($imgHeight -gt 1400) {{ $imgHeight = 1400.0 }}
    }}

    $stream = New-Object Windows.Storage.Streams.InMemoryRandomAccessStream
    $opts = New-Object Windows.Data.Pdf.PdfPageRenderOptions
    $opts.DestinationWidth = [uint32][Math]::Round($imgWidth)
    $opts.DestinationHeight = [uint32][Math]::Round($imgHeight)
    & $awaitAction ($page.RenderToStreamAsync($stream, $opts))
    $stream.Seek(0)
    $inStream = $stream.GetInputStreamAt(0)
    $netStream = [System.IO.WindowsRuntimeStreamExtensions]::AsStreamForRead($inStream)
    $ms = New-Object System.IO.MemoryStream
    $netStream.CopyTo($ms)
    $bytes = $ms.ToArray()
    $bmp = [System.Drawing.Image]::FromStream((New-Object System.IO.MemoryStream(,$bytes)))
  }} catch {{
    $imgWidth = 850.0
    $imgHeight = 1100.0
    $bmp = New-Object System.Drawing.Bitmap ([int]$imgWidth), ([int]$imgHeight)
    $g0 = [System.Drawing.Graphics]::FromImage($bmp)
    $g0.Clear([System.Drawing.Color]::WhiteSmoke)
    $pen0 = New-Object System.Drawing.Pen ([System.Drawing.Color]::Gray), 2
    $g0.DrawRectangle($pen0, 1, 1, [int]$imgWidth - 3, [int]$imgHeight - 3)
    $font0 = New-Object System.Drawing.Font('Segoe UI', 14, [System.Drawing.FontStyle]::Bold)
    $brush0 = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::DimGray)
    $g0.DrawString('Preview do PDF indisponivel neste ambiente.', $font0, $brush0, 26, 26)
    $g0.Dispose()
  }}

  $sizes = Get-PresetSize $placement
  $rw = [double]$sizes[0]
  $rh = [double]$sizes[1]
  if ($rw -gt $imgWidth * 0.85) {{ $rw = [Math]::Max(40.0, $imgWidth * 0.85) }}
  if ($rh -gt $imgHeight * 0.85) {{ $rh = [Math]::Max(40.0, $imgHeight * 0.85) }}
  $x = ($imgWidth - $rw) / 2.0
  $y = ($imgHeight - $rh) / 2.0

  $previewForm = New-Object System.Windows.Forms.Form
  $previewForm.Text = 'Posicionamento manual da assinatura'
  $previewForm.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
  $previewForm.Width = [Math]::Min(980, [int]$imgWidth + 70)
  $previewForm.Height = [Math]::Min(900, [int]$imgHeight + 160)

  $pic = New-Object System.Windows.Forms.PictureBox
  $pic.Left = 18
  $pic.Top = 18
  $pic.Width = [int]$imgWidth
  $pic.Height = [int]$imgHeight
  $pic.SizeMode = [System.Windows.Forms.PictureBoxSizeMode]::Normal
  $pic.Image = $bmp
  $pic.BorderStyle = [System.Windows.Forms.BorderStyle]::FixedSingle

  $lblHelp = New-Object System.Windows.Forms.Label
  $lblHelp.Left = 18
  $lblHelp.Top = $pic.Bottom + 10
  $lblHelp.Width = [Math]::Min(860, $pic.Width)
  $lblHelp.Height = 24
  $lblHelp.Text = 'Arraste o retangulo laranja para escolher a posicao na primeira pagina.'

  $btnUse = New-Object System.Windows.Forms.Button
  $btnUse.Text = 'Usar esta posicao'
  $btnUse.Left = $pic.Right - 220
  $btnUse.Top = $lblHelp.Bottom + 2
  $btnUse.Width = 140

  $btnCancelPreview = New-Object System.Windows.Forms.Button
  $btnCancelPreview.Text = 'Cancelar'
  $btnCancelPreview.Left = $pic.Right - 72
  $btnCancelPreview.Top = $lblHelp.Bottom + 2
  $btnCancelPreview.Width = 72

  $dragging = $false
  $dragOffsetX = 0.0
  $dragOffsetY = 0.0
  $rect = New-Object System.Drawing.RectangleF ([single]$x), ([single]$y), ([single]$rw), ([single]$rh)

  $pic.Add_Paint({
    param($sender, $e)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::OrangeRed), 3
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(60, [System.Drawing.Color]::Orange))
    $e.Graphics.FillRectangle($brush, $rect)
    $e.Graphics.DrawRectangle($pen, $rect.X, $rect.Y, $rect.Width, $rect.Height)
    $pen.Dispose()
    $brush.Dispose()
  })

  $pic.Add_MouseDown({
    param($sender, $e)
    if ($e.Button -ne [System.Windows.Forms.MouseButtons]::Left) { return }
    if ($rect.Contains([single]$e.X, [single]$e.Y)) {
      $dragging = $true
      $dragOffsetX = [double]$e.X - [double]$rect.X
      $dragOffsetY = [double]$e.Y - [double]$rect.Y
    }
  })

  $pic.Add_MouseMove({
    param($sender, $e)
    if (-not $dragging) { return }
    $newX = [double]$e.X - $dragOffsetX
    $newY = [double]$e.Y - $dragOffsetY
    $newX = Clamp $newX 0.0 ($imgWidth - [double]$rect.Width)
    $newY = Clamp $newY 0.0 ($imgHeight - [double]$rect.Height)
    $rect.X = [single]$newX
    $rect.Y = [single]$newY
    $pic.Invalidate()
  })

  $pic.Add_MouseUp({
    param($sender, $e)
    if ($e.Button -eq [System.Windows.Forms.MouseButtons]::Left) {
      $dragging = $false
    }
  })

  $previewForm.Controls.Add($pic)
  $previewForm.Controls.Add($lblHelp)
  $previewForm.Controls.Add($btnUse)
  $previewForm.Controls.Add($btnCancelPreview)

  $resultRect = $null
  $btnUse.Add_Click({
    $x0 = [double]$rect.X / $imgWidth
    $x1 = ([double]$rect.X + [double]$rect.Width) / $imgWidth
    $yBottom = 1.0 - (([double]$rect.Y + [double]$rect.Height) / $imgHeight)
    $yTop = 1.0 - ([double]$rect.Y / $imgHeight)
    $resultRect = @(
      [Math]::Round($x0, 6),
      [Math]::Round($yBottom, 6),
      [Math]::Round($x1, 6),
      [Math]::Round($yTop, 6)
    )
    $previewForm.DialogResult = [System.Windows.Forms.DialogResult]::OK
    $previewForm.Close()
  })

  $btnCancelPreview.Add_Click({
    $previewForm.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
    $previewForm.Close()
  })

  $pr = $previewForm.ShowDialog()
  if ($pr -eq [System.Windows.Forms.DialogResult]::OK -and $resultRect) {
    return $resultRect
  }
  return $null
}

function Update-VisibleControls() {
  $enabled = $chkVisible.Checked
  $comboPlacement.Enabled = $enabled
  $chkManual.Enabled = $enabled
  $btnManual.Enabled = $enabled -and $chkManual.Checked
}

$chkVisible.Add_CheckedChanged({
  if (-not $chkVisible.Checked) {
    $script:manualRect = $null
    $lblManualStatus.Text = 'Posicao manual nao definida.'
  }
  Update-VisibleControls
})

$chkManual.Add_CheckedChanged({
  if (-not $chkManual.Checked) {
    $script:manualRect = $null
    $lblManualStatus.Text = 'Posicao manual nao definida.'
  }
  Update-VisibleControls
})

$btnManual.Add_Click({
  if (-not $comboPlacement.SelectedItem) {
    [System.Windows.Forms.MessageBox]::Show('Selecione primeiro uma posicao base.', 'Posicionamento manual', [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Information) | Out-Null
    return
  }
  $placementCurrent = $comboPlacement.SelectedItem.ToString()
  $rect = Show-ManualPlacementDialog $previewPdfPath $placementCurrent
  if ($rect) {
    $script:manualRect = $rect
    $lblManualStatus.Text = 'Posicao manual definida.'
  }
})

Update-VisibleControls

$btnOk = New-Object System.Windows.Forms.Button
$btnOk.Text = 'Assinar'
$btnOk.Left = 600
$btnOk.Top = 266
$btnOk.Width = 90
$btnOk.Add_Click({
  if ($combo.SelectedIndex -ge 0) {
    if ($chkVisible.Checked -and $chkManual.Checked -and -not $script:manualRect) {
      [System.Windows.Forms.MessageBox]::Show('Defina a posicao manual antes de continuar.', 'Posicionamento manual', [System.Windows.Forms.MessageBoxButtons]::OK, [System.Windows.Forms.MessageBoxIcon]::Warning) | Out-Null
      return
    }
    $placement = $null
    if ($chkVisible.Checked -and $comboPlacement.SelectedItem) {
      $placement = $comboPlacement.SelectedItem.ToString()
    }
    $manualRectOut = $null
    if ($chkVisible.Checked -and $chkManual.Checked -and $script:manualRect) {
      $manualRectOut = $script:manualRect
    }
    $payload = @{
      use_auto = ($combo.SelectedIndex -eq 0)
      index = [Math]::Max($combo.SelectedIndex - 1, 0)
      visible_signature = $chkVisible.Checked
      placement = $placement
      manual_rect = $manualRectOut
    } | ConvertTo-Json -Compress
    $form.Tag = $payload
    $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
  }
  $form.Close()
})

$btnCancel = New-Object System.Windows.Forms.Button
$btnCancel.Text = 'Cancelar'
$btnCancel.Left = 698
$btnCancel.Top = 266
$btnCancel.Width = 90
$btnCancel.Add_Click({
  $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
  $form.Close()
})

$form.Controls.Add($label)
$form.Controls.Add($combo)
$form.Controls.Add($chkVisible)
$form.Controls.Add($labelPos)
$form.Controls.Add($comboPlacement)
$form.Controls.Add($chkManual)
$form.Controls.Add($btnManual)
$form.Controls.Add($lblManualStatus)
$form.Controls.Add($btnOk)
$form.Controls.Add($btnCancel)
$form.AcceptButton = $btnOk
$form.CancelButton = $btnCancel

$result = $form.ShowDialog()
if ($result -eq [System.Windows.Forms.DialogResult]::OK -and $form.Tag) {
  Write-Output $form.Tag
}
"#;
    let script = script_template
        .replace("__PAYLOAD_B64__", &payload_b64)
        .replace("__PREVIEW_B64__", &preview_b64)
        .replace(
            "__PRESELECTED_POSITION__",
            &preselected_position.to_string(),
        )
        .replace("{{", "{")
        .replace("}}", "}");

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("Falha ao abrir dialogo de selecao de certificado")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Dialogo de selecao retornou erro: {}", stderr.trim());
    }

    let selected_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if selected_raw.is_empty() {
        return Ok(None);
    }

    let parsed: TrayDropdownResult = serde_json::from_str(&selected_raw)
        .context("Retorno invalido do dialogo de selecao de certificado")?;
    Ok(Some(parsed))
}
