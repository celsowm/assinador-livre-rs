use crate::{
    contracts::{CertificateSummary, VisibleSignatureRequest},
    logger,
    runtime::{
        CertDialogInput, CertDialogOutput, DesktopRuntime, INSTANCE_MUTEX_NAME,
        SingleInstanceGuard, TrayCommand, TrayGuard, UiMessageLevel,
    },
};
use anyhow::{Context, Result, bail};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use single_instance::SingleInstance;
use std::{path::PathBuf, process::Command, sync::mpsc::Sender};

pub struct UnixRuntime;

impl UnixRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl DesktopRuntime for UnixRuntime {
    fn single_instance_guard(&self) -> Result<Box<dyn SingleInstanceGuard>> {
        let instance = SingleInstance::new(INSTANCE_MUTEX_NAME)
            .context("Falha ao criar mutex de instancia unica")?;
        if !instance.is_single() {
            bail!("Outra instancia do Assinador Livre ja esta em execucao.");
        }
        Ok(Box::new(instance))
    }

    fn create_tray(&self, command_tx: Sender<TrayCommand>) -> Result<Box<dyn TrayGuard>> {
        let _ = command_tx;
        logger::warn(
            "Tray nativo ainda nao implementado para este sistema operacional (modo headless).",
        );
        Ok(Box::new(UnixTrayHandle))
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
        FileDialog::new()
            .set_title("Selecione os PDFs para assinar")
            .add_filter("Arquivos PDF", &["pdf"])
            .pick_files()
            .unwrap_or_default()
    }

    fn choose_certificate_and_visible_signature(
        &self,
        input: CertDialogInput,
    ) -> Result<Option<CertDialogOutput>> {
        let _candidates: Vec<CertificateSummary> = input.candidates;
        let _visible: Option<VisibleSignatureRequest> = None;
        Ok(Some(CertDialogOutput {
            cert_selection: None,
            visible_signature: None,
        }))
    }

    fn open_url(&self, url: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(url)
                .spawn()
                .context("Falha ao abrir URL no navegador")?;
            return Ok(());
        }

        #[cfg(not(target_os = "macos"))]
        {
            Command::new("xdg-open")
                .arg(url)
                .spawn()
                .context("Falha ao abrir URL no navegador")?;
            return Ok(());
        }
    }

    fn set_startup(&self, enabled: bool) -> Result<()> {
        logger::warn(format!(
            "startup_with_os_login={} ainda nao implementado para este sistema operacional",
            enabled
        ));
        Ok(())
    }
}

struct UnixTrayHandle;
