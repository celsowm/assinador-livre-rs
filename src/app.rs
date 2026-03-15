use crate::{
    config::{AppConfig, LoadedConfig},
    logger, signer,
    tray::{self, TrayCommand},
    ws,
};
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rfd::{MessageButtons, MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};
use single_instance::SingleInstance;
use std::{env, os::windows::process::CommandExt, process::Command, sync::Arc};
use tokio::sync::Semaphore;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const INSTANCE_MUTEX_NAME: &str = "Global\\AssinadorLivreMutex";
const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE_NAME: &str = "AssinadorLivre";

#[derive(Clone)]
pub struct SharedState {
    pub config: AppConfig,
    pub verbose: bool,
    pub signing_gate: Arc<Semaphore>,
}

impl SharedState {
    fn new(config: AppConfig, verbose: bool) -> Self {
        Self {
            config,
            verbose,
            signing_gate: Arc::new(Semaphore::new(1)),
        }
    }
}

#[derive(Default)]
struct CliArgs {
    sign_now: bool,
    print_config_path: bool,
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = parse_args();
    let loaded = crate::config::load_or_create()?;

    if args.print_config_path {
        println!("{}", loaded.paths.config_path.display());
        return Ok(());
    }

    logger::init(loaded.paths.log_file.clone(), args.verbose)?;
    logger::info("Inicializando Assinador Livre");

    if args.sign_now {
        return run_sign_now(&loaded, args.verbose);
    }

    run_tray_mode(loaded, args.verbose)
}

fn run_sign_now(loaded: &LoadedConfig, verbose: bool) -> Result<()> {
    logger::info("Modo --sign-now iniciado");
    let report = signer::sign_selected_files(&loaded.config.cert_override, verbose)?;

    logger::info(format!(
        "Modo --sign-now finalizado: {} sucesso(s), {} erro(s)",
        report.signed.len(),
        report.errors.len()
    ));

    Ok(())
}

fn run_tray_mode(loaded: LoadedConfig, verbose: bool) -> Result<()> {
    let instance = SingleInstance::new(INSTANCE_MUTEX_NAME)
        .context("Falha ao criar mutex de instancia unica")?;
    if !instance.is_single() {
        bail!("Outra instancia do Assinador Livre ja esta em execucao.");
    }

    if let Err(e) = ensure_startup_entry(loaded.config.startup_with_windows) {
        logger::warn(format!("Falha ao atualizar auto-start: {e:#}"));
    }

    let state = Arc::new(SharedState::new(loaded.config.clone(), verbose));

    let mut ws_server =
        ws::spawn_server(state.clone()).context("Falha ao iniciar servidor WebSocket local")?;

    logger::info(format!(
        "Servidor WebSocket local: {}",
        loaded.config.endpoint()
    ));

    let (command_tx, command_rx) = std::sync::mpsc::channel::<TrayCommand>();
    let _tray = tray::create_tray(command_tx).context("Falha ao inicializar bandeja")?;

    logger::info("App iniciado em modo bandeja");

    loop {
        match command_rx.recv() {
            Ok(TrayCommand::SignDocument) => {
                handle_sign_from_tray(&state);
            }
            Ok(TrayCommand::OpenPlayground) => {
                open_playground_from_tray(&state);
            }
            Ok(TrayCommand::Exit) => {
                logger::info("Comando de saida recebido pela bandeja");
                break;
            }
            Err(_) => {
                logger::warn("Canal da bandeja foi encerrado");
                break;
            }
        }
    }

    ws_server.shutdown();
    drop(instance);
    logger::info("Aplicacao encerrada");

    Ok(())
}

fn handle_sign_from_tray(state: &Arc<SharedState>) {
    let permit = match state.signing_gate.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            logger::warn("Assinatura ignorada: app ocupado");
            MessageDialog::new()
                .set_title("Assinatura em andamento")
                .set_description(
                    "Ja existe uma assinatura em andamento. Tente novamente em instantes.",
                )
                .set_level(MessageLevel::Warning)
                .set_buttons(MessageButtons::Ok)
                .show();
            return;
        }
    };

    let tray_choice = match choose_tray_certificate_selection(state) {
        Ok(choice) => choice,
        Err(e) => {
            logger::error(format!(
                "Falha na selecao de certificado via bandeja: {e:#}"
            ));
            MessageDialog::new()
                .set_title("Erro na selecao de certificado")
                .set_description(format!("{e:#}"))
                .set_level(MessageLevel::Error)
                .set_buttons(MessageButtons::Ok)
                .show();
            drop(permit);
            return;
        }
    };

    let (cert_selection, visible_signature) = match tray_choice {
        TrayCertificateChoice::Selected {
            cert_selection,
            visible_signature,
        } => (cert_selection, visible_signature),
        TrayCertificateChoice::Cancelled => {
            logger::info("Assinatura via bandeja cancelada pelo usuario");
            drop(permit);
            return;
        }
    };

    logger::info("Assinatura iniciada via bandeja");

    if let Err(e) = signer::sign_selected_files_with_selection(
        &state.config.cert_override,
        state.verbose,
        cert_selection,
        visible_signature,
    ) {
        logger::error(format!("Falha na assinatura via bandeja: {e:#}"));
        MessageDialog::new()
            .set_title("Erro na assinatura")
            .set_description(format!("{e:#}"))
            .set_level(MessageLevel::Error)
            .set_buttons(MessageButtons::Ok)
            .show();
    }

    drop(permit);
}

enum TrayCertificateChoice {
    Selected {
        cert_selection: Option<signer::CertSelectionRequest>,
        visible_signature: Option<signer::VisibleSignatureRequest>,
    },
    Cancelled,
}

fn choose_tray_certificate_selection(state: &SharedState) -> Result<TrayCertificateChoice> {
    let certs = signer::list_available_certificates()?;
    if certs.is_empty() {
        bail!("Nenhum certificado disponivel para assinatura.");
    }

    let mode = state.config.cert_override.mode.as_str();
    let candidates: Vec<&signer::CertificateSummary> = if mode == "token_only" {
        certs.iter().filter(|cert| cert.is_hardware_token).collect()
    } else {
        certs.iter().collect()
    };

    if candidates.is_empty() {
        bail!(
            "Nenhum certificado elegivel para selecao manual no modo '{}'.",
            mode
        );
    }

    let recommended_index =
        signer::recommended_certificate_index(&state.config.cert_override, state.verbose).ok();
    let preselected_position = recommended_index
        .and_then(|idx| candidates.iter().position(|cert| cert.index == idx))
        .unwrap_or(0);

    let selected = show_certificate_dropdown_dialog(&candidates, preselected_position)?;
    let Some(selected) = selected else {
        return Ok(TrayCertificateChoice::Cancelled);
    };

    let cert_selection = if selected.use_auto {
        None
    } else {
        if selected.index >= candidates.len() {
            bail!(
                "Indice selecionado fora da faixa: {} (max {})",
                selected.index,
                candidates.len().saturating_sub(1)
            );
        }
        let cert = candidates[selected.index];
        Some(signer::CertSelectionRequest {
            thumbprint: Some(cert.thumbprint.clone()),
            index: Some(cert.index),
        })
    };
    let visible_signature = if selected.visible_signature {
        let placement_raw = selected
            .placement
            .as_deref()
            .unwrap_or("bottom_center_horizontal");
        let placement = parse_visible_signature_placement(placement_raw).ok_or_else(|| {
            anyhow::anyhow!(
                "Posicao de assinatura visivel invalida retornada pelo dialogo: '{}'",
                placement_raw
            )
        })?;
        Some(signer::VisibleSignatureRequest {
            placement,
            style: signer::VisibleSignatureStyle::Default,
            timezone: signer::VisibleSignatureTimezone::Local,
        })
    } else {
        None
    };

    Ok(TrayCertificateChoice::Selected {
        cert_selection,
        visible_signature,
    })
}

fn parse_visible_signature_placement(raw: &str) -> Option<signer::VisibleSignaturePlacement> {
    match raw {
        "top_left_horizontal" => Some(signer::VisibleSignaturePlacement::TopLeftHorizontal),
        "top_left_vertical" => Some(signer::VisibleSignaturePlacement::TopLeftVertical),
        "top_right_horizontal" => Some(signer::VisibleSignaturePlacement::TopRightHorizontal),
        "top_right_vertical" => Some(signer::VisibleSignaturePlacement::TopRightVertical),
        "bottom_left_horizontal" => Some(signer::VisibleSignaturePlacement::BottomLeftHorizontal),
        "bottom_left_vertical" => Some(signer::VisibleSignaturePlacement::BottomLeftVertical),
        "bottom_right_horizontal" => Some(signer::VisibleSignaturePlacement::BottomRightHorizontal),
        "bottom_right_vertical" => Some(signer::VisibleSignaturePlacement::BottomRightVertical),
        "bottom_center_horizontal" => {
            Some(signer::VisibleSignaturePlacement::BottomCenterHorizontal)
        }
        "bottom_center_vertical" => Some(signer::VisibleSignaturePlacement::BottomCenterVertical),
        "center_horizontal" => Some(signer::VisibleSignaturePlacement::CenterHorizontal),
        "center_vertical" => Some(signer::VisibleSignaturePlacement::CenterVertical),
        _ => None,
    }
}

#[derive(Deserialize)]
struct TrayDropdownResult {
    use_auto: bool,
    index: usize,
    visible_signature: bool,
    placement: Option<String>,
}

#[derive(Serialize)]
struct TrayDropdownItem {
    display: String,
}

fn show_certificate_dropdown_dialog(
    candidates: &[&signer::CertificateSummary],
    preselected_position: usize,
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
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$json = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{payload_b64}'))
$items = $json | ConvertFrom-Json

$form = New-Object System.Windows.Forms.Form
$form.Text = 'Selecao de certificado'
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
$form.Width = 760
$form.Height = 280
$form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
$form.MaximizeBox = $false
$form.MinimizeBox = $false

$label = New-Object System.Windows.Forms.Label
$label.Left = 18
$label.Top = 20
$label.Width = 710
$label.Height = 36
$label.Text = 'Escolha o certificado para assinatura (pre-selecionado pelo algoritmo atual):'

$combo = New-Object System.Windows.Forms.ComboBox
$combo.Left = 18
$combo.Top = 62
$combo.Width = 710
$combo.DropDownStyle = [System.Windows.Forms.ComboBoxStyle]::DropDownList
[void]$combo.Items.Add('automatico (algoritmo atual)')
foreach ($item in $items) {{ [void]$combo.Items.Add($item.display) }}

$pre = {preselected_position}
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

$chkVisible.Add_CheckedChanged({{
  $comboPlacement.Enabled = $chkVisible.Checked
}})

$btnOk = New-Object System.Windows.Forms.Button
$btnOk.Text = 'Assinar'
$btnOk.Left = 540
$btnOk.Top = 190
$btnOk.Width = 90
$btnOk.Add_Click({{
  if ($combo.SelectedIndex -ge 0) {{
    $placement = $null
    if ($chkVisible.Checked -and $comboPlacement.SelectedItem) {{
      $placement = $comboPlacement.SelectedItem.ToString()
    }}
    $payload = @{{
      use_auto = ($combo.SelectedIndex -eq 0)
      index = [Math]::Max($combo.SelectedIndex - 1, 0)
      visible_signature = $chkVisible.Checked
      placement = $placement
    }} | ConvertTo-Json -Compress
    $form.Tag = $payload
    $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
  }}
  $form.Close()
}})

$btnCancel = New-Object System.Windows.Forms.Button
$btnCancel.Text = 'Cancelar'
$btnCancel.Left = 638
$btnCancel.Top = 190
$btnCancel.Width = 90
$btnCancel.Add_Click({{
  $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
  $form.Close()
}})

$form.Controls.Add($label)
$form.Controls.Add($combo)
$form.Controls.Add($chkVisible)
$form.Controls.Add($labelPos)
$form.Controls.Add($comboPlacement)
$form.Controls.Add($btnOk)
$form.Controls.Add($btnCancel)
$form.AcceptButton = $btnOk
$form.CancelButton = $btnCancel

$result = $form.ShowDialog()
if ($result -eq [System.Windows.Forms.DialogResult]::OK -and $form.Tag) {{
  Write-Output $form.Tag
}}
"#
    );

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

fn open_playground_from_tray(state: &Arc<SharedState>) {
    let url = format!(
        "http://{}:{}/playground",
        state.config.ws_host, state.config.ws_port
    );

    logger::info(format!("Abrindo playground no navegador: {url}"));

    let open_result = Command::new("cmd").args(["/C", "start", "", &url]).spawn();

    if let Err(e) = open_result {
        logger::error(format!("Falha ao abrir playground no navegador: {e:#}"));
        MessageDialog::new()
            .set_title("Erro ao abrir playground")
            .set_description(format!(
                "Nao foi possivel abrir o navegador automaticamente.\nURL: {url}\n\nErro: {e:#}"
            ))
            .set_level(MessageLevel::Error)
            .set_buttons(MessageButtons::Ok)
            .show();
    }
}

fn parse_args() -> CliArgs {
    let mut args = CliArgs::default();
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--sign-now" => args.sign_now = true,
            "--print-config-path" => args.print_config_path = true,
            "--verbose" | "-v" => args.verbose = true,
            _ => {}
        }
    }
    args
}

fn ensure_startup_entry(enabled: bool) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = hkcu
        .create_subkey(RUN_KEY_PATH)
        .context("Falha ao abrir chave Run")?;

    if enabled {
        let exe_path = env::current_exe().context("Falha ao descobrir caminho do executavel")?;
        let value = format!("\"{}\"", exe_path.display());
        run_key
            .set_value(RUN_VALUE_NAME, &value)
            .context("Falha ao escrever entrada de inicializacao")?;
    } else {
        let _ = run_key.delete_value(RUN_VALUE_NAME);
    }

    Ok(())
}
