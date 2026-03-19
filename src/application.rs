use crate::{
    config::AppConfig,
    contracts::{
        BatchFileInput, BatchSignResult, CertSelectionRequest, CertificateSummary,
        SIGNING_BACKEND_UNAVAILABLE, SignReport, TraySigningRequest, VisibleSignatureRequest,
        WsSignResult,
    },
    signer_backend::SignerBackend,
};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct AppService {
    pub config: AppConfig,
    pub verbose: bool,
    pub signing_gate: Arc<Semaphore>,
    backend: Arc<dyn SignerBackend>,
}

#[derive(Debug)]
pub enum AppServiceError {
    Invalid(String),
    Signing(String),
    BackendUnavailable(String),
}

impl AppService {
    pub fn new(config: AppConfig, verbose: bool, backend: Arc<dyn SignerBackend>) -> Self {
        Self {
            config,
            verbose,
            signing_gate: Arc::new(Semaphore::new(1)),
            backend,
        }
    }

    pub fn list_certificates(
        &self,
    ) -> std::result::Result<Vec<CertificateSummary>, AppServiceError> {
        self.backend
            .list_certificates()
            .map_err(classify_backend_error)
    }

    pub fn recommended_certificate_index(&self) -> Result<usize> {
        self.backend
            .recommended_certificate_index(&self.config.cert_override, self.verbose)
    }

    pub fn sign_single_pdf(
        &self,
        input: &[u8],
        visible_signature: Option<VisibleSignatureRequest>,
        cert_selection: Option<CertSelectionRequest>,
    ) -> std::result::Result<WsSignResult, AppServiceError> {
        self.backend
            .sign_single_pdf(
                input,
                &self.config.cert_override,
                self.verbose,
                visible_signature,
                cert_selection,
            )
            .map_err(classify_backend_error)
    }

    pub fn sign_batch_pdfs(
        &self,
        inputs: Vec<BatchFileInput>,
        cert_selection: Option<CertSelectionRequest>,
    ) -> std::result::Result<BatchSignResult, AppServiceError> {
        self.backend
            .sign_batch_pdfs(
                inputs,
                &self.config.cert_override,
                self.verbose,
                cert_selection,
            )
            .map_err(classify_backend_error)
    }

    pub fn sign_tray_request(
        &self,
        request: TraySigningRequest,
    ) -> std::result::Result<SignReport, AppServiceError> {
        self.backend
            .sign_tray_request(request, &self.config.cert_override, self.verbose)
            .map_err(classify_backend_error)
    }

    pub fn playground_url(&self) -> String {
        format!(
            "http://{}:{}/playground",
            self.config.ws_host, self.config.ws_port
        )
    }
}

pub fn classify_backend_error(err: anyhow::Error) -> AppServiceError {
    let msg = format!("{err:#}");
    if msg.contains(SIGNING_BACKEND_UNAVAILABLE) {
        return AppServiceError::BackendUnavailable(msg);
    }
    if msg.contains("cert_thumbprint")
        || msg.contains("cert_index")
        || msg.contains("Payload")
        || msg.contains("invalido")
    {
        return AppServiceError::Invalid(msg);
    }
    AppServiceError::Signing(msg)
}
