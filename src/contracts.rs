use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const SIGNING_BACKEND_UNAVAILABLE: &str = "SIGNING_BACKEND_UNAVAILABLE";

#[derive(Debug, Clone, Serialize)]
pub struct CertificateSummary {
    pub index: usize,
    pub subject: String,
    pub issuer: String,
    pub thumbprint: String,
    pub is_hardware_token: bool,
    pub provider_name: String,
    pub valid_now: bool,
    pub supports_digital_signature: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CertSelectionRequest {
    pub thumbprint: Option<String>,
    pub index: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibleSignaturePlacement {
    TopLeftHorizontal,
    TopLeftVertical,
    TopRightHorizontal,
    TopRightVertical,
    BottomLeftHorizontal,
    BottomLeftVertical,
    BottomRightHorizontal,
    BottomRightVertical,
    BottomCenterHorizontal,
    BottomCenterVertical,
    CenterHorizontal,
    CenterVertical,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VisibleSignatureStyle {
    #[default]
    Default,
    Compact,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VisibleSignatureTimezone {
    Utc,
    #[default]
    Local,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VisibleSignatureRequest {
    pub placement: VisibleSignaturePlacement,
    #[serde(default)]
    pub custom_rect: Option<[f32; 4]>,
    #[serde(default)]
    pub style: VisibleSignatureStyle,
    #[serde(default)]
    pub timezone: VisibleSignatureTimezone,
}

pub struct WsSignResult {
    pub signed_pdf: Vec<u8>,
    pub cert_subject: String,
    pub cert_issuer: String,
    pub cert_thumbprint: String,
    pub cert_is_hardware_token: bool,
    pub cert_provider_name: String,
}

pub struct BatchFileInput {
    pub filename: String,
    pub pdf_bytes: Vec<u8>,
    pub visible_signature: Option<VisibleSignatureRequest>,
}

pub struct BatchFileResult {
    pub filename: String,
    pub ok: bool,
    pub signed_pdf: Option<Vec<u8>>,
    pub error: Option<String>,
}

pub struct BatchSignResult {
    pub files: Vec<BatchFileResult>,
    pub cert_subject: String,
    pub cert_issuer: String,
    pub cert_thumbprint: String,
    pub cert_is_hardware_token: bool,
    pub cert_provider_name: String,
}

pub struct SignReport {
    pub signed: Vec<String>,
    pub errors: Vec<String>,
}

pub struct TraySigningRequest {
    pub pdfs: Vec<PathBuf>,
    pub cert_selection: Option<CertSelectionRequest>,
    pub visible_signature: Option<VisibleSignatureRequest>,
}
