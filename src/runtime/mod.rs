#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::WindowsRuntime as DefaultRuntime;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::UnixRuntime as DefaultRuntime;

use crate::contracts::{CertSelectionRequest, CertificateSummary, VisibleSignatureRequest};
use anyhow::Result;
use std::{path::PathBuf, sync::mpsc::Sender};

pub const INSTANCE_MUTEX_NAME: &str = "Global\\AssinadorLivreMutex";

#[derive(Debug, Clone, Copy)]
pub enum TrayCommand {
    SignDocument,
    OpenPlayground,
    Exit,
}

#[derive(Debug, Clone, Copy)]
pub enum UiMessageLevel {
    Warning,
    Error,
}

pub struct CertDialogInput {
    pub candidates: Vec<CertificateSummary>,
    pub preselected_position: usize,
    pub preview_pdf: Option<PathBuf>,
}

pub struct CertDialogOutput {
    pub cert_selection: Option<CertSelectionRequest>,
    pub visible_signature: Option<VisibleSignatureRequest>,
}

pub trait DesktopRuntime: Send + Sync {
    fn single_instance_guard(&self) -> Result<Box<dyn SingleInstanceGuard>>;
    fn create_tray(&self, command_tx: Sender<TrayCommand>) -> Result<Box<dyn TrayGuard>>;
    fn show_message(&self, level: UiMessageLevel, title: &str, description: &str);
    fn pick_pdfs(&self) -> Vec<PathBuf>;
    fn choose_certificate_and_visible_signature(
        &self,
        input: CertDialogInput,
    ) -> Result<Option<CertDialogOutput>>;
    fn open_url(&self, url: &str) -> Result<()>;
    fn set_startup(&self, enabled: bool) -> Result<()>;
}

pub trait SingleInstanceGuard {}
impl<T> SingleInstanceGuard for T {}

pub trait TrayGuard {}
impl<T> TrayGuard for T {}

pub fn create_default_runtime() -> DefaultRuntime {
    DefaultRuntime::new()
}
