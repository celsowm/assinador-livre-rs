use crate::{
    application::{AppService, AppServiceError},
    config::LoadedConfig,
    contracts::{TraySigningRequest, VisibleSignatureRequest},
    logger,
    runtime::{
        CertDialogInput, DesktopRuntime, TrayCommand, UiMessageLevel, create_default_runtime,
    },
    signer_backend, ws,
};
use anyhow::{Context, Result};
use std::{env, sync::Arc};

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

    let runtime: Arc<dyn DesktopRuntime> = Arc::new(create_default_runtime());
    let backend = signer_backend::default_backend();
    let service = Arc::new(AppService::new(
        loaded.config.clone(),
        args.verbose,
        backend,
    ));

    if args.sign_now {
        return run_sign_now(&loaded, service, runtime);
    }

    run_tray_mode(service, runtime)
}

fn run_sign_now(
    loaded: &LoadedConfig,
    service: Arc<AppService>,
    runtime: Arc<dyn DesktopRuntime>,
) -> Result<()> {
    logger::info("Modo --sign-now iniciado");
    let pdfs = runtime.pick_pdfs();
    if pdfs.is_empty() {
        logger::info("Modo --sign-now cancelado: nenhum PDF selecionado");
        return Ok(());
    }

    match service.sign_tray_request(TraySigningRequest {
        pdfs,
        cert_selection: None,
        visible_signature: None,
    }) {
        Ok(report) => {
            logger::info(format!(
                "Modo --sign-now finalizado: {} sucesso(s), {} erro(s)",
                report.signed.len(),
                report.errors.len()
            ));
            Ok(())
        }
        Err(err) => {
            logger::error(format!("Falha no modo --sign-now: {err:?}"));
            runtime.show_message(
                UiMessageLevel::Error,
                "Erro na assinatura",
                &format_service_error(&err),
            );
            if matches!(err, AppServiceError::BackendUnavailable(_)) {
                logger::warn(format!(
                    "Backend de assinatura indisponivel fora Windows para config {}",
                    loaded.paths.config_path.display()
                ));
            }
            Ok(())
        }
    }
}

fn run_tray_mode(service: Arc<AppService>, runtime: Arc<dyn DesktopRuntime>) -> Result<()> {
    let _instance = runtime.single_instance_guard()?;

    if let Err(e) = runtime.set_startup(service.config.startup_with_os_login) {
        logger::warn(format!("Falha ao atualizar auto-start: {e:#}"));
    }

    let mut ws_server =
        ws::spawn_server(service.clone()).context("Falha ao iniciar servidor WebSocket local")?;

    logger::info(format!(
        "Servidor WebSocket local: {}",
        service.config.endpoint()
    ));

    let (command_tx, command_rx) = std::sync::mpsc::channel::<TrayCommand>();
    let _tray = runtime
        .create_tray(command_tx)
        .context("Falha ao inicializar bandeja")?;

    logger::info("App iniciado em modo bandeja");

    loop {
        match command_rx.recv() {
            Ok(TrayCommand::SignDocument) => {
                handle_sign_from_tray(service.clone(), runtime.clone());
            }
            Ok(TrayCommand::OpenPlayground) => {
                open_playground_from_tray(service.clone(), runtime.clone());
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
    logger::info("Aplicacao encerrada");

    Ok(())
}

fn handle_sign_from_tray(service: Arc<AppService>, runtime: Arc<dyn DesktopRuntime>) {
    let permit = match service.signing_gate.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            logger::warn("Assinatura ignorada: app ocupado");
            runtime.show_message(
                UiMessageLevel::Warning,
                "Assinatura em andamento",
                "Ja existe uma assinatura em andamento. Tente novamente em instantes.",
            );
            return;
        }
    };

    let pdfs = runtime.pick_pdfs();
    if pdfs.is_empty() {
        logger::info("Assinatura via bandeja cancelada: nenhum PDF selecionado");
        drop(permit);
        return;
    }

    let certs = match service.list_certificates() {
        Ok(certs) => certs,
        Err(err) => {
            logger::error(format!("Falha ao listar certificados: {err:?}"));
            runtime.show_message(
                UiMessageLevel::Error,
                "Erro na selecao de certificado",
                &format_service_error(&err),
            );
            drop(permit);
            return;
        }
    };

    let mode = service.config.cert_override.mode.as_str();
    let candidates: Vec<_> = if mode == "token_only" {
        certs
            .into_iter()
            .filter(|cert| cert.is_hardware_token)
            .collect()
    } else {
        certs
    };

    if candidates.is_empty() {
        runtime.show_message(
            UiMessageLevel::Error,
            "Erro na selecao de certificado",
            &format!(
                "Nenhum certificado elegivel para selecao manual no modo '{}'.",
                mode
            ),
        );
        drop(permit);
        return;
    }

    let preselected_position = service
        .recommended_certificate_index()
        .ok()
        .and_then(|idx| candidates.iter().position(|cert| cert.index == idx))
        .unwrap_or(0);

    let choice = match runtime.choose_certificate_and_visible_signature(CertDialogInput {
        candidates,
        preselected_position,
        preview_pdf: pdfs.first().cloned(),
    }) {
        Ok(result) => result,
        Err(err) => {
            logger::error(format!(
                "Falha na selecao de certificado via bandeja: {err:#}"
            ));
            runtime.show_message(
                UiMessageLevel::Error,
                "Erro na selecao de certificado",
                &format!("{err:#}"),
            );
            drop(permit);
            return;
        }
    };

    let Some(choice) = choice else {
        logger::info("Assinatura via bandeja cancelada pelo usuario");
        drop(permit);
        return;
    };

    logger::info("Assinatura iniciada via bandeja");

    if let Err(err) = service.sign_tray_request(TraySigningRequest {
        pdfs,
        cert_selection: choice.cert_selection,
        visible_signature: choice.visible_signature,
    }) {
        logger::error(format!("Falha na assinatura via bandeja: {err:?}"));
        runtime.show_message(
            UiMessageLevel::Error,
            "Erro na assinatura",
            &format_service_error(&err),
        );
    }

    drop(permit);
}

fn open_playground_from_tray(service: Arc<AppService>, runtime: Arc<dyn DesktopRuntime>) {
    let url = service.playground_url();
    logger::info(format!("Abrindo playground no navegador: {url}"));

    if let Err(e) = runtime.open_url(&url) {
        logger::error(format!("Falha ao abrir playground no navegador: {e:#}"));
        runtime.show_message(
            UiMessageLevel::Error,
            "Erro ao abrir playground",
            &format!(
                "Nao foi possivel abrir o navegador automaticamente.\nURL: {url}\n\nErro: {e:#}"
            ),
        );
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

fn format_service_error(err: &AppServiceError) -> String {
    match err {
        AppServiceError::Invalid(msg)
        | AppServiceError::Signing(msg)
        | AppServiceError::BackendUnavailable(msg) => msg.clone(),
    }
}

#[allow(dead_code)]
fn _normalize_visible_signature(
    value: Option<VisibleSignatureRequest>,
) -> Option<VisibleSignatureRequest> {
    value
}
