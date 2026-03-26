use crate::{
    config::CertOverride,
    contracts::{
        BatchFileInput, BatchSignResult, CertSelectionRequest, CertificateSummary, SignReport,
        TraySigningRequest, VisibleSignatureRequest, WsSignResult,
        VisibleSignaturePlacement, VisibleSignatureStyle, VisibleSignatureTimezone,
    },
};
use anyhow::Result;
use std::sync::Arc;

pub trait SignerBackend: Send + Sync {
    fn list_certificates(&self) -> Result<Vec<CertificateSummary>>;
    fn recommended_certificate_index(
        &self,
        cert_override: &CertOverride,
        verbose: bool,
    ) -> Result<usize>;
    fn sign_single_pdf(
        &self,
        input: &[u8],
        cert_override: &CertOverride,
        verbose: bool,
        visible_signature: Option<VisibleSignatureRequest>,
        cert_selection: Option<CertSelectionRequest>,
    ) -> Result<WsSignResult>;
    fn sign_batch_pdfs(
        &self,
        inputs: Vec<BatchFileInput>,
        cert_override: &CertOverride,
        verbose: bool,
        cert_selection: Option<CertSelectionRequest>,
    ) -> Result<BatchSignResult>;
    fn sign_tray_request(
        &self,
        request: TraySigningRequest,
        cert_override: &CertOverride,
        verbose: bool,
    ) -> Result<SignReport>;
}

pub fn default_backend() -> Arc<dyn SignerBackend> {
    Arc::new(GenericSignerBackend)
}

struct GenericSignerBackend;

impl SignerBackend for GenericSignerBackend {
    fn list_certificates(&self) -> Result<Vec<CertificateSummary>> {
        let certs = crate::signer::list_available_certificates()?;
        Ok(certs
            .into_iter()
            .map(|c| CertificateSummary {
                index: c.index,
                subject: c.subject,
                issuer: c.issuer,
                thumbprint: c.thumbprint,
                is_hardware_token: c.is_hardware_token,
                provider_name: c.provider_name,
                valid_now: c.valid_now,
                supports_digital_signature: c.supports_digital_signature,
            })
            .collect())
    }

    fn recommended_certificate_index(
        &self,
        cert_override: &CertOverride,
        verbose: bool,
    ) -> Result<usize> {
        crate::signer::recommended_certificate_index(cert_override, verbose)
    }

    fn sign_single_pdf(
        &self,
        input: &[u8],
        cert_override: &CertOverride,
        verbose: bool,
        visible_signature: Option<VisibleSignatureRequest>,
        cert_selection: Option<CertSelectionRequest>,
    ) -> Result<WsSignResult> {
        let result = crate::signer::sign_single_pdf_bytes(
            input,
            cert_override,
            verbose,
            visible_signature.map(to_signer_visible_signature),
            cert_selection.map(to_signer_cert_selection),
            None, // WS currently doesn't support PIN
        )?;

        Ok(WsSignResult {
            signed_pdf: result.signed_pdf,
            cert_subject: result.cert_subject,
            cert_issuer: result.cert_issuer,
            cert_thumbprint: result.cert_thumbprint,
            cert_is_hardware_token: result.cert_is_hardware_token,
            cert_provider_name: result.cert_provider_name,
        })
    }

    fn sign_batch_pdfs(
        &self,
        inputs: Vec<BatchFileInput>,
        cert_override: &CertOverride,
        verbose: bool,
        cert_selection: Option<CertSelectionRequest>,
    ) -> Result<BatchSignResult> {
        let signer_inputs = inputs
            .into_iter()
            .map(|entry| crate::signer::BatchFileInput {
                filename: entry.filename,
                pdf_bytes: entry.pdf_bytes,
                visible_signature: entry.visible_signature.map(to_signer_visible_signature),
            })
            .collect();

        let result = crate::signer::sign_batch_pdf_bytes(
            signer_inputs,
            cert_override,
            verbose,
            cert_selection.map(to_signer_cert_selection),
            None, // WS currently doesn't support PIN
        )?;

        Ok(BatchSignResult {
            files: result
                .files
                .into_iter()
                .map(|f| crate::contracts::BatchFileResult {
                    filename: f.filename,
                    ok: f.ok,
                    signed_pdf: f.signed_pdf,
                    error: f.error,
                })
                .collect(),
            cert_subject: result.cert_subject,
            cert_issuer: result.cert_issuer,
            cert_thumbprint: result.cert_thumbprint,
            cert_is_hardware_token: result.cert_is_hardware_token,
            cert_provider_name: result.cert_provider_name,
        })
    }

    fn sign_tray_request(
        &self,
        request: TraySigningRequest,
        cert_override: &CertOverride,
        verbose: bool,
    ) -> Result<SignReport> {
        let report = crate::signer::sign_pdfs_with_selection(
            request.pdfs,
            cert_override,
            verbose,
            request.cert_selection.map(to_signer_cert_selection),
            request.visible_signature.map(to_signer_visible_signature),
            request.pin,
        )?;

        Ok(SignReport {
            signed: report.signed,
            errors: report.errors,
        })
    }
}

fn to_signer_cert_selection(req: CertSelectionRequest) -> crate::signer::CertSelectionRequest {
    crate::signer::CertSelectionRequest {
        thumbprint: req.thumbprint,
        index: req.index,
    }
}

fn to_signer_visible_signature(
    req: VisibleSignatureRequest,
) -> crate::signer::VisibleSignatureRequest {
    crate::signer::VisibleSignatureRequest {
        placement: to_signer_placement(req.placement),
        custom_rect: req.custom_rect,
        style: to_signer_style(req.style),
        timezone: to_signer_timezone(req.timezone),
    }
}

fn to_signer_placement(
    placement: VisibleSignaturePlacement,
) -> crate::signer::VisibleSignaturePlacement {
    match placement {
        VisibleSignaturePlacement::TopLeftHorizontal => {
            crate::signer::VisibleSignaturePlacement::TopLeftHorizontal
        }
        VisibleSignaturePlacement::TopLeftVertical => {
            crate::signer::VisibleSignaturePlacement::TopLeftVertical
        }
        VisibleSignaturePlacement::TopRightHorizontal => {
            crate::signer::VisibleSignaturePlacement::TopRightHorizontal
        }
        VisibleSignaturePlacement::TopRightVertical => {
            crate::signer::VisibleSignaturePlacement::TopRightVertical
        }
        VisibleSignaturePlacement::BottomLeftHorizontal => {
            crate::signer::VisibleSignaturePlacement::BottomLeftHorizontal
        }
        VisibleSignaturePlacement::BottomLeftVertical => {
            crate::signer::VisibleSignaturePlacement::BottomLeftVertical
        }
        VisibleSignaturePlacement::BottomRightHorizontal => {
            crate::signer::VisibleSignaturePlacement::BottomRightHorizontal
        }
        VisibleSignaturePlacement::BottomRightVertical => {
            crate::signer::VisibleSignaturePlacement::BottomRightVertical
        }
        VisibleSignaturePlacement::BottomCenterHorizontal => {
            crate::signer::VisibleSignaturePlacement::BottomCenterHorizontal
        }
        VisibleSignaturePlacement::BottomCenterVertical => {
            crate::signer::VisibleSignaturePlacement::BottomCenterVertical
        }
        VisibleSignaturePlacement::CenterHorizontal => {
            crate::signer::VisibleSignaturePlacement::CenterHorizontal
        }
        VisibleSignaturePlacement::CenterVertical => {
            crate::signer::VisibleSignaturePlacement::CenterVertical
        }
    }
}

fn to_signer_style(style: VisibleSignatureStyle) -> crate::signer::VisibleSignatureStyle {
    match style {
        VisibleSignatureStyle::Default => crate::signer::VisibleSignatureStyle::Default,
        VisibleSignatureStyle::Compact => crate::signer::VisibleSignatureStyle::Compact,
    }
}

fn to_signer_timezone(tz: VisibleSignatureTimezone) -> crate::signer::VisibleSignatureTimezone {
    match tz {
        VisibleSignatureTimezone::Utc => crate::signer::VisibleSignatureTimezone::Utc,
        VisibleSignatureTimezone::Local => crate::signer::VisibleSignatureTimezone::Local,
    }
}
