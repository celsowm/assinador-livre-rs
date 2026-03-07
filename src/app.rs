use crate::{
    config::{AppConfig, LoadedConfig},
    logger, signer,
    tray::{self, TrayCommand},
    ws,
};
use anyhow::{bail, Context, Result};
use rfd::{MessageButtons, MessageDialog, MessageLevel};
use single_instance::SingleInstance;
use std::{env, process::Command, sync::Arc};
use tokio::sync::Semaphore;
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

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

    logger::info("Assinatura iniciada via bandeja");

    if let Err(e) = signer::sign_selected_files(&state.config.cert_override, state.verbose) {
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

fn open_playground_from_tray(state: &Arc<SharedState>) {
    let url = format!(
        "http://{}:{}/playground",
        state.config.ws_host, state.config.ws_port
    );

    logger::info(format!("Abrindo playground no navegador: {url}"));

    let open_result = Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();

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
