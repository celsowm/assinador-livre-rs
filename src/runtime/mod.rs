use anyhow::Result;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use single_instance::SingleInstance;
use std::{path::PathBuf, process::Command};

pub const INSTANCE_MUTEX_NAME: &str = "Global\\AssinadorLivreMutex";

#[derive(Debug, Clone, Copy)]
pub enum UiMessageLevel {
    Warning,
    Error,
}

pub trait DesktopRuntime: Send + Sync {
    fn single_instance_guard(&self) -> Result<Box<dyn SingleInstanceGuard>>;
    fn show_message(&self, level: UiMessageLevel, title: &str, description: &str);
    fn pick_pdfs(&self) -> Vec<PathBuf>;
    fn open_url(&self, url: &str) -> Result<()>;
}

pub trait SingleInstanceGuard {}
impl<T> SingleInstanceGuard for T {}

pub struct DefaultRuntime;

pub fn create_default_runtime() -> DefaultRuntime {
    DefaultRuntime
}

impl DesktopRuntime for DefaultRuntime {
    fn single_instance_guard(&self) -> Result<Box<dyn SingleInstanceGuard>> {
        let instance = SingleInstance::new(INSTANCE_MUTEX_NAME)?;
        if !instance.is_single() {
            return Err(anyhow::anyhow!(
                "Outra instancia do Assinador Livre ja esta em execucao."
            ));
        }
        Ok(Box::new(instance))
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
        let mut dialog = FileDialog::new()
            .set_title("Selecione os PDFs para assinar")
            .add_filter("Arquivos PDF", &["pdf"]);

        #[cfg(windows)]
        {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                dialog = dialog.set_directory(PathBuf::from(profile).join("Desktop"));
            }
        }

        dialog.pick_files().unwrap_or_default()
    }

    fn open_url(&self, url: &str) -> Result<()> {
        #[cfg(target_os = "windows")]
        {
            Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            Command::new("open").arg(url).spawn()?;
            return Ok(());
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Command::new("xdg-open").arg(url).spawn()?;
            return Ok(());
        }
    }
}
