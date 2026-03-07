use crate::{config::CertOverride, logger};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use lopdf::{Document, Object, ObjectId};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use serde::Deserialize;
use std::{
    env, fs,
    io::{self, Write as _},
    mem,
    path::{Path, PathBuf},
    slice,
};
use windows::{
    core::{w, PSTR},
    Win32::{Foundation::BOOL, Security::Cryptography::*},
};

pub struct SignReport {
    pub signed: Vec<String>,
    pub errors: Vec<String>,
}

pub struct WsSignResult {
    pub signed_pdf: Vec<u8>,
    pub cert_subject: String,
    pub cert_issuer: String,
    pub cert_thumbprint: String,
    pub cert_is_hardware_token: bool,
    pub cert_provider_name: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct VisibleSignatureRequest {
    pub placement: VisibleSignaturePlacement,
}

#[derive(Debug, Clone)]
pub struct VisibleSignatureAppearance {
    pub placement: VisibleSignaturePlacement,
    pub signer_name: String,
}

pub struct OwnedCert {
    pub subject: String,
    pub issuer: String,
    pub thumbprint: String,
    pub context: *const CERT_CONTEXT,
    pub valid_now: bool,
    pub supports_digital_signature: bool,
    pub key_provider_name: String,
    pub key_container_name: String,
    pub key_provider_type: u32,
    pub key_spec: u32,
    pub is_hardware_token: bool,
}

unsafe impl Send for OwnedCert {}
unsafe impl Sync for OwnedCert {}

impl Drop for OwnedCert {
    fn drop(&mut self) {
        unsafe {
            let _ = CertFreeCertificateContext(Some(self.context));
        }
    }
}

pub fn sign_selected_files(cert_override: &CertOverride, verbose: bool) -> Result<SignReport> {
    let pdfs = select_pdfs();
    if pdfs.is_empty() {
        return Ok(SignReport {
            signed: Vec::new(),
            errors: Vec::new(),
        });
    }

    let certs = load_available_certificates()?;
    let cert_idx = choose_certificate_index(&certs, cert_override, verbose)?;
    let cert = &certs[cert_idx];

    let mut report = SignReport {
        signed: Vec::new(),
        errors: Vec::new(),
    };

    for (i, input) in pdfs.iter().enumerate() {
        let nome = file_name(input);
        let output = output_name(input);
        print!("[{}/{}] Assinando: {}... ", i + 1, pdfs.len(), nome);
        io::stdout().flush().ok();

        match sign_pdf_file(input, &output, cert.context) {
            Ok(()) => {
                println!("OK");
                report.signed.push(nome);
            }
            Err(e) => {
                println!("ERRO: {e}");
                report.errors.push(format!("{nome}: {e}"));
            }
        }
    }

    show_summary(&pdfs, &report);
    Ok(report)
}

pub fn sign_single_pdf_bytes(
    input: &[u8],
    cert_override: &CertOverride,
    verbose: bool,
    visible_signature: Option<VisibleSignatureRequest>,
) -> Result<WsSignResult> {
    let certs = load_available_certificates()?;
    let cert_idx = choose_certificate_index(&certs, cert_override, verbose)?;
    let cert = &certs[cert_idx];

    let visible_signature = visible_signature.map(|cfg| VisibleSignatureAppearance {
        placement: cfg.placement,
        signer_name: cert.subject.clone(),
    });

    let signed_pdf = sign_pdf_bytes(input, cert.context, visible_signature.as_ref())?;

    Ok(WsSignResult {
        signed_pdf,
        cert_subject: cert.subject.clone(),
        cert_issuer: cert.issuer.clone(),
        cert_thumbprint: cert.thumbprint.clone(),
        cert_is_hardware_token: cert.is_hardware_token,
        cert_provider_name: cert.key_provider_name.clone(),
    })
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

pub fn sign_batch_pdf_bytes(
    inputs: Vec<BatchFileInput>,
    cert_override: &CertOverride,
    verbose: bool,
) -> Result<BatchSignResult> {
    let certs = load_available_certificates()?;
    let cert_idx = choose_certificate_index(&certs, cert_override, verbose)?;
    let cert = &certs[cert_idx];

    let mut file_results = Vec::with_capacity(inputs.len());

    for input in &inputs {
        let visible_signature = input.visible_signature.as_ref().map(|cfg| VisibleSignatureAppearance {
            placement: cfg.placement,
            signer_name: cert.subject.clone(),
        });

        match sign_pdf_bytes(&input.pdf_bytes, cert.context, visible_signature.as_ref()) {
            Ok(signed_pdf) => {
                file_results.push(BatchFileResult {
                    filename: input.filename.clone(),
                    ok: true,
                    signed_pdf: Some(signed_pdf),
                    error: None,
                });
            }
            Err(e) => {
                file_results.push(BatchFileResult {
                    filename: input.filename.clone(),
                    ok: false,
                    signed_pdf: None,
                    error: Some(format!("{e:#}")),
                });
            }
        }
    }

    Ok(BatchSignResult {
        files: file_results,
        cert_subject: cert.subject.clone(),
        cert_issuer: cert.issuer.clone(),
        cert_thumbprint: cert.thumbprint.clone(),
        cert_is_hardware_token: cert.is_hardware_token,
        cert_provider_name: cert.key_provider_name.clone(),
    })
}

pub fn sign_pdf_file(input: &Path, output: &Path, cert_ctx: *const CERT_CONTEXT) -> Result<()> {
    let original = fs::read(input)?;
    let signed = sign_pdf_bytes(&original, cert_ctx, None)
        .with_context(|| format!("Falha ao assinar {}", input.display()))?;
    fs::write(output, signed).with_context(|| format!("Falha ao gravar {}", output.display()))?;
    Ok(())
}

pub fn sign_pdf_bytes(
    input: &[u8],
    cert_ctx: *const CERT_CONTEXT,
    visible_signature: Option<&VisibleSignatureAppearance>,
) -> Result<Vec<u8>> {
    const SIG_BYTES: usize = 12_288;
    const HEX_LEN: usize = SIG_BYTES * 2;
    const BR_PLACEHOLDER: &[u8] = b"/ByteRange [0 AAAAAAAAAA BBBBBBBBBB CCCCCCCCCC]";

    let original = input.to_vec();
    let doc = Document::load_mem(input).context("Falha ao abrir PDF em memoria")?;
    let signed_at = Utc::now();

    let (cat_num, cat_gen) = doc
        .trailer
        .get(b"Root")
        .context("Trailer sem /Root")?
        .as_reference()
        .context("/Root nao e referencia")?;

    let catalog = doc
        .get_object((cat_num, cat_gen))
        .context("/Catalog nao encontrado")?
        .as_dict()
        .context("/Catalog nao e dicionario")?;

    let first_page_id = first_page_object_id(&doc)?;
    let first_page_media_box = resolve_page_media_box(&doc, first_page_id)?;
    let mut first_page_dict = doc
        .get_object(first_page_id)
        .context("Primeira pagina nao encontrada")?
        .as_dict()
        .context("Primeira pagina nao e dicionario")?
        .clone();

    let mut existing_fields = existing_acroform_fields(&doc, catalog);
    let next_obj = next_free_obj_num(&original)?;
    let sig_num = next_obj;
    let fld_num = next_obj + 1;
    let af_num = next_obj + 2;
    let mut next_dynamic = af_num + 1;

    let font_num = visible_signature.map(|_| {
        let v = next_dynamic;
        next_dynamic += 1;
        v
    });
    let ap_num = visible_signature.map(|_| {
        let v = next_dynamic;
        next_dynamic += 1;
        v
    });

    let widget_rect = visible_signature
        .map(|cfg| compute_visible_signature_rect(first_page_media_box, cfg.placement))
        .unwrap_or([0.0, 0.0, 0.0, 0.0]);

    let mut annots = extract_page_annots(&doc, &first_page_dict);
    annots.push(Object::Reference((fld_num, 0)));
    first_page_dict.set("Annots", Object::Array(annots));
    existing_fields.push(Object::Reference((fld_num, 0)));

    let mut upd = Vec::<u8>::new();
    let mut xref_entries = Vec::<XrefEntry>::new();

    xref_entries.push(XrefEntry::new(sig_num, 0, original.len() + upd.len()));
    write!(upd, "{sig_num} 0 obj\n<<\n")?;
    write!(upd, "/Type /Sig\n")?;
    write!(upd, "/Filter /Adobe.PPKLite\n")?;
    write!(upd, "/SubFilter /adbe.pkcs7.detached\n")?;
    upd.extend_from_slice(BR_PLACEHOLDER);
    upd.push(b'\n');
    write!(upd, "/Contents <")?;
    upd.extend(std::iter::repeat(b'0').take(HEX_LEN));
    write!(upd, ">\n")?;
    write!(upd, "/Reason (Assinado digitalmente com Token A3)\n")?;
    write!(upd, "/Location (Brasil)\n")?;
    write!(upd, "/M (D:{})\n", signed_at.format("%Y%m%d%H%M%S+00'00'"))?;
    write!(upd, ">>\nendobj\n")?;

    xref_entries.push(XrefEntry::new(fld_num, 0, original.len() + upd.len()));
    write!(upd, "{fld_num} 0 obj\n<<\n")?;
    write!(upd, "/Type /Annot\n/Subtype /Widget\n/FT /Sig\n")?;
    write!(upd, "/T (Assinatura_Digital_A3)\n")?;
    write!(upd, "/V {sig_num} 0 R\n")?;
    write!(
        upd,
        "/Rect [{:.2} {:.2} {:.2} {:.2}]\n",
        widget_rect[0], widget_rect[1], widget_rect[2], widget_rect[3]
    )?;
    write!(upd, "/P {} {} R\n", first_page_id.0, first_page_id.1)?;
    if let Some(ap_num) = ap_num {
        write!(upd, "/F 4\n")?;
        write!(upd, "/AP << /N {ap_num} 0 R >>\n")?;
    }
    write!(upd, ">>\nendobj\n")?;

    xref_entries.push(XrefEntry::new(af_num, 0, original.len() + upd.len()));
    let all_fields = existing_fields
        .iter()
        .map(|obj| format!("{obj:?}"))
        .collect::<Vec<String>>()
        .join(" ");
    write!(upd, "{af_num} 0 obj\n<<\n")?;
    write!(upd, "/Fields [{all_fields}]\n/SigFlags 3\n")?;
    write!(upd, ">>\nendobj\n")?;

    if let Some(font_num) = font_num {
        xref_entries.push(XrefEntry::new(font_num, 0, original.len() + upd.len()));
        write!(upd, "{font_num} 0 obj\n<<\n")?;
        write!(upd, "/Type /Font\n/Subtype /Type1\n/BaseFont /Helvetica\n")?;
        write!(upd, ">>\nendobj\n")?;
    }

    if let (Some(ap_num), Some(font_num), Some(visible_cfg)) = (ap_num, font_num, visible_signature)
    {
        let appearance =
            build_visible_signature_appearance(widget_rect, &visible_cfg.signer_name, signed_at);
        xref_entries.push(XrefEntry::new(ap_num, 0, original.len() + upd.len()));
        write!(upd, "{ap_num} 0 obj\n<<\n")?;
        write!(upd, "/Type /XObject\n/Subtype /Form\n")?;
        write!(
            upd,
            "/BBox [0 0 {:.2} {:.2}]\n",
            widget_rect[2] - widget_rect[0],
            widget_rect[3] - widget_rect[1]
        )?;
        write!(upd, "/Resources << /Font << /F1 {font_num} 0 R >> >>\n")?;
        write!(upd, "/Length {}\n", appearance.len())?;
        write!(upd, ">>\nstream\n")?;
        upd.extend_from_slice(&appearance);
        write!(upd, "\nendstream\nendobj\n")?;
    }

    xref_entries.push(XrefEntry::new(
        first_page_id.0,
        first_page_id.1,
        original.len() + upd.len(),
    ));
    write!(upd, "{} {} obj\n", first_page_id.0, first_page_id.1)?;
    write!(upd, "{:?}\n", Object::Dictionary(first_page_dict))?;
    write!(upd, "endobj\n")?;

    let mut catalog_updated = catalog.clone();
    catalog_updated.set("AcroForm", Object::Reference((af_num, 0)));
    xref_entries.push(XrefEntry::new(cat_num, cat_gen, original.len() + upd.len()));
    write!(upd, "{cat_num} {cat_gen} obj\n")?;
    write!(upd, "{:?}\n", Object::Dictionary(catalog_updated))?;
    write!(upd, "endobj\n")?;

    let xref_off = original.len() + upd.len();
    write!(upd, "\nxref\n")?;
    xref_entries.sort_by_key(|entry| entry.obj_num);

    let mut idx = 0usize;
    while idx < xref_entries.len() {
        let start = xref_entries[idx].obj_num;
        let mut end = idx + 1;
        while end < xref_entries.len()
            && xref_entries[end].obj_num == xref_entries[end - 1].obj_num + 1
        {
            end += 1;
        }
        write!(upd, "{start} {}\n", end - idx)?;
        for entry in &xref_entries[idx..end] {
            write!(upd, "{:010} {:05} n \n", entry.offset, entry.generation)?;
        }
        idx = end;
    }

    write!(upd, "trailer\n<<\n")?;
    write!(upd, "/Size {}\n", next_dynamic)?;
    write!(upd, "/Root {cat_num} {cat_gen} R\n")?;
    write!(upd, "/Prev {}\n", original.len())?;
    write!(upd, ">>\nstartxref\n{xref_off}\n%%EOF\n")?;

    let mut pdf = original.clone();
    pdf.extend_from_slice(&upd);

    let b1 = find_contents_hex_start(&pdf, original.len())
        .context("Placeholder /Contents nao localizado no PDF provisorio")?;
    let b2 = b1 + 1 + HEX_LEN + 1;
    let b3 = pdf.len() - b2;

    let br_new = format!("/ByteRange [0 {b1:<10} {b2:<10} {b3:<10}]");
    assert_eq!(
        br_new.len(),
        BR_PLACEHOLDER.len(),
        "ByteRange placeholder com comprimento incorreto - verifique BR_PLACEHOLDER"
    );

    let br_pos = find_subsequence(&pdf[original.len()..], BR_PLACEHOLDER)
        .context("ByteRange placeholder nao localizado (busca)")?
        + original.len();
    pdf[br_pos..br_pos + BR_PLACEHOLDER.len()].copy_from_slice(br_new.as_bytes());

    let signed_bytes: Vec<u8> = [&pdf[..b1], &pdf[b2..]].concat();
    let cms_der = unsafe { cms_sign_detached(cert_ctx, &signed_bytes)? };

    if cms_der.len() > SIG_BYTES {
        bail!(
            "Assinatura CMS tem {} bytes; limite reservado e {SIG_BYTES}. Aumente SIG_BYTES e recompile.",
            cms_der.len()
        );
    }

    let hex: String = cms_der.iter().map(|b| format!("{b:02X}")).collect();
    let hex_padded: String = format!("{hex:0<HEX_LEN$}");

    let contents_pos = find_contents_hex_start(&pdf, original.len())
        .context("Placeholder /Contents nao localizado (2a passagem)")?;
    pdf[contents_pos..contents_pos + HEX_LEN].copy_from_slice(hex_padded.as_bytes());

    Ok(pdf)
}

#[derive(Debug, Clone, Copy)]
struct XrefEntry {
    obj_num: u32,
    generation: u16,
    offset: usize,
}

impl XrefEntry {
    fn new(obj_num: u32, generation: u16, offset: usize) -> Self {
        Self {
            obj_num,
            generation,
            offset,
        }
    }
}

fn first_page_object_id(doc: &Document) -> Result<ObjectId> {
    doc.get_pages()
        .into_iter()
        .next()
        .map(|(_, id)| id)
        .context("PDF nao possui paginas")
}

fn existing_acroform_fields(doc: &Document, catalog: &lopdf::Dictionary) -> Vec<Object> {
    match catalog.get(b"AcroForm") {
        Ok(acro_form) => {
            let acro_dict = if let Ok(id) = acro_form.as_reference() {
                doc.get_object(id).ok().and_then(|o| o.as_dict().ok())
            } else {
                acro_form.as_dict().ok()
            };

            acro_dict
                .and_then(|d| d.get(b"Fields").ok())
                .and_then(|f| f.as_array().ok())
                .cloned()
                .unwrap_or_default()
        }
        Err(_) => Vec::new(),
    }
}

fn resolve_page_media_box(doc: &Document, page_id: ObjectId) -> Result<[f32; 4]> {
    let mut current = Some(page_id);
    while let Some(object_id) = current {
        let dict = doc
            .get_object(object_id)
            .context("Objeto de pagina nao encontrado")?
            .as_dict()
            .context("Objeto de pagina nao e dicionario")?;

        if let Ok(media_box) = dict.get(b"MediaBox") {
            let array = if let Ok(reference) = media_box.as_reference() {
                doc.get_object(reference)
                    .context("Referencia de MediaBox nao encontrada")?
                    .as_array()
                    .context("MediaBox referenciado nao e array")?
                    .clone()
            } else {
                media_box
                    .as_array()
                    .context("MediaBox nao e array")?
                    .clone()
            };
            return parse_rect_array(&array);
        }

        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }

    bail!("Primeira pagina sem MediaBox")
}

fn parse_rect_array(arr: &[Object]) -> Result<[f32; 4]> {
    if arr.len() != 4 {
        bail!(
            "Retangulo invalido: esperado 4 numeros, recebido {}",
            arr.len()
        );
    }

    Ok([
        object_to_f32(&arr[0])?,
        object_to_f32(&arr[1])?,
        object_to_f32(&arr[2])?,
        object_to_f32(&arr[3])?,
    ])
}

fn object_to_f32(obj: &Object) -> Result<f32> {
    match obj {
        Object::Integer(v) => Ok(*v as f32),
        Object::Real(v) => Ok(*v),
        _ => bail!("Valor numerico invalido em retangulo"),
    }
}

fn extract_page_annots(doc: &Document, page_dict: &lopdf::Dictionary) -> Vec<Object> {
    let annots_obj = match page_dict.get(b"Annots") {
        Ok(obj) => obj,
        Err(_) => return Vec::new(),
    };

    if let Ok(array) = annots_obj.as_array() {
        return array.clone();
    }

    if let Ok(reference) = annots_obj.as_reference()
        && let Ok(array) = doc.get_object(reference).and_then(|o| o.as_array())
    {
        return array.clone();
    }

    Vec::new()
}

fn compute_visible_signature_rect(
    media_box: [f32; 4],
    placement: VisibleSignaturePlacement,
) -> [f32; 4] {
    const MARGIN: f32 = 24.0;
    const H_WIDTH: f32 = 220.0;
    const H_HEIGHT: f32 = 72.0;
    const V_WIDTH: f32 = 110.0;
    const V_HEIGHT: f32 = 180.0;

    let (target_w, target_h) = match placement {
        VisibleSignaturePlacement::TopLeftHorizontal
        | VisibleSignaturePlacement::TopRightHorizontal
        | VisibleSignaturePlacement::BottomLeftHorizontal
        | VisibleSignaturePlacement::BottomRightHorizontal => (H_WIDTH, H_HEIGHT),
        VisibleSignaturePlacement::TopLeftVertical
        | VisibleSignaturePlacement::TopRightVertical
        | VisibleSignaturePlacement::BottomLeftVertical
        | VisibleSignaturePlacement::BottomRightVertical => (V_WIDTH, V_HEIGHT),
    };

    let llx = media_box[0];
    let lly = media_box[1];
    let urx = media_box[2];
    let ury = media_box[3];

    let usable_w = (urx - llx - 2.0 * MARGIN).max(20.0);
    let usable_h = (ury - lly - 2.0 * MARGIN).max(20.0);
    let width = target_w.min(usable_w);
    let height = target_h.min(usable_h);

    let x = match placement {
        VisibleSignaturePlacement::TopLeftHorizontal
        | VisibleSignaturePlacement::TopLeftVertical
        | VisibleSignaturePlacement::BottomLeftHorizontal
        | VisibleSignaturePlacement::BottomLeftVertical => llx + MARGIN,
        VisibleSignaturePlacement::TopRightHorizontal
        | VisibleSignaturePlacement::TopRightVertical
        | VisibleSignaturePlacement::BottomRightHorizontal
        | VisibleSignaturePlacement::BottomRightVertical => urx - MARGIN - width,
    };

    let y = match placement {
        VisibleSignaturePlacement::TopLeftHorizontal
        | VisibleSignaturePlacement::TopLeftVertical
        | VisibleSignaturePlacement::TopRightHorizontal
        | VisibleSignaturePlacement::TopRightVertical => ury - MARGIN - height,
        VisibleSignaturePlacement::BottomLeftHorizontal
        | VisibleSignaturePlacement::BottomLeftVertical
        | VisibleSignaturePlacement::BottomRightHorizontal
        | VisibleSignaturePlacement::BottomRightVertical => lly + MARGIN,
    };

    [x, y, x + width, y + height]
}

fn build_visible_signature_appearance(
    rect: [f32; 4],
    signer_name: &str,
    signed_at: DateTime<Utc>,
) -> Vec<u8> {
    let width = rect[2] - rect[0];
    let height = rect[3] - rect[1];
    let line1 = escape_pdf_literal("Assinado digitalmente");
    let line2 = escape_pdf_literal(&truncate_text(
        &format!("Assinante: {}", signer_name.trim()),
        80,
    ));
    let line3 = escape_pdf_literal(&format!(
        "Data/Hora: {}",
        signed_at.format("%d/%m/%Y %H:%M:%S UTC")
    ));
    let baseline = (height - 18.0).max(20.0);

    format!(
        "q\n1 1 0.93 rg\n0 0 {width:.2} {height:.2} re\nf\n0 0 0 RG\n1 w\n0 0 {width:.2} {height:.2} re\nS\nBT\n/F1 11 Tf\n0 0 0 rg\n8 {baseline:.2} Td\n({line1}) Tj\n0 -15 Td\n({line2}) Tj\n0 -15 Td\n({line3}) Tj\nET\nQ\n"
    )
    .into_bytes()
}

fn truncate_text(input: &str, limit: usize) -> String {
    input.chars().take(limit).collect()
}

fn escape_pdf_literal(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

pub fn normalize_thumbprint(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn load_available_certificates() -> Result<Vec<OwnedCert>> {
    let certs = list_my_certificates()
        .context("Falha ao acessar o repositorio de certificados do Windows")?;

    if certs.is_empty() {
        bail!(
            "Nenhum certificado com chave privada encontrado no repositorio 'Minhas'.\n\
             Verifique se o driver do token A3 esta instalado e o dispositivo conectado."
        );
    }

    Ok(certs)
}

fn choose_certificate_index(
    certs: &[OwnedCert],
    cert_override: &CertOverride,
    verbose: bool,
) -> Result<usize> {
    if let Some(index) = cert_override.index {
        if (1..=certs.len()).contains(&index) {
            let selected = index - 1;
            ensure_mode_allows_certificate(&certs[selected], &cert_override.mode)?;
            log_certificate_selection(certs, selected, "config.index");
            return Ok(selected);
        }
        bail!(
            "cert_override.index invalido: {}. Valores aceitos: 1..={}",
            index,
            certs.len()
        );
    }

    if let Some(tp) = cert_override.thumbprint.as_ref() {
        let wanted = normalize_thumbprint(tp);
        if !wanted.is_empty() {
            if let Some((idx, _)) = certs
                .iter()
                .enumerate()
                .find(|(_, cert)| cert.thumbprint == wanted)
            {
                ensure_mode_allows_certificate(&certs[idx], &cert_override.mode)?;
                log_certificate_selection(certs, idx, "config.thumbprint");
                return Ok(idx);
            }
            eprintln!(
                "[WARN] thumbprint '{}' nao encontrado. Aplicando selecao automatica.",
                wanted
            );
        }
    }

    if let Ok(raw) = env::var("ASSINADOR_CERT_INDEX") {
        let idx = raw.trim().parse::<usize>().unwrap_or(0);
        if (1..=certs.len()).contains(&idx) {
            let selected = idx - 1;
            ensure_mode_allows_certificate(&certs[selected], &cert_override.mode)?;
            log_certificate_selection(certs, selected, "env.ASSINADOR_CERT_INDEX");
            return Ok(selected);
        }
        bail!(
            "ASSINADOR_CERT_INDEX invalido: '{}'. Valores aceitos: 1..={}.",
            raw.trim(),
            certs.len()
        );
    }

    if certs.len() == 1 {
        ensure_mode_allows_certificate(&certs[0], &cert_override.mode)?;
        log_certificate_selection(certs, 0, "auto.single");
        return Ok(0);
    }

    let base_candidates: Vec<usize> = certs
        .iter()
        .enumerate()
        .filter_map(|(idx, cert)| (!is_test_certificate(cert)).then_some(idx))
        .collect();
    let mut candidate_indexes = if base_candidates.is_empty() {
        (0..certs.len()).collect::<Vec<usize>>()
    } else {
        base_candidates
    };

    let hardware_candidates: Vec<usize> = candidate_indexes
        .iter()
        .copied()
        .filter(|idx| certs[*idx].is_hardware_token)
        .collect();

    match cert_override.mode.as_str() {
        "token_only" => {
            if hardware_candidates.is_empty() {
                bail!(
                    "cert_override.mode=token_only exige certificado de token/smart card, \
                     mas nenhum foi encontrado no repositorio 'Minhas'."
                );
            }
            candidate_indexes = hardware_candidates;
        }
        "auto" => {
            if !hardware_candidates.is_empty() {
                candidate_indexes = hardware_candidates;
            }
        }
        _ => {
            bail!(
                "cert_override.mode invalido: '{}'. Valores aceitos: auto, token_only",
                cert_override.mode
            );
        }
    }

    let ranked = rank_certificates(certs, &candidate_indexes);
    let best_idx = ranked[0].index;

    log_certificate_selection(certs, best_idx, "auto");

    if verbose {
        println!("[AUTO][verbose] Motivos por certificado:");
        for entry in &ranked {
            println!(
                "  [{}] score={} {} | token_hardware={} | provider='{}'",
                entry.index + 1,
                entry.score,
                certs[entry.index].subject,
                certs[entry.index].is_hardware_token,
                certs[entry.index].key_provider_name
            );
            for reason in &entry.reasons {
                println!("       - {reason}");
            }
        }
    }

    Ok(best_idx)
}

struct RankedCert {
    index: usize,
    score: i32,
    reasons: Vec<String>,
}

fn rank_certificates(certs: &[OwnedCert], candidate_indexes: &[usize]) -> Vec<RankedCert> {
    let mut ranked: Vec<RankedCert> = candidate_indexes
        .iter()
        .map(|idx| {
            let (score, reasons) = certificate_score(&certs[*idx]);
            RankedCert {
                index: *idx,
                score,
                reasons,
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
    ranked
}

fn certificate_score(cert: &OwnedCert) -> (i32, Vec<String>) {
    let subject_lc = cert.subject.to_ascii_lowercase();
    let issuer_lc = cert.issuer.to_ascii_lowercase();
    let provider_lc = cert.key_provider_name.to_ascii_lowercase();
    let container_lc = cert.key_container_name.to_ascii_lowercase();

    let mut score = 0;
    let mut reasons = Vec::new();
    if !cert.subject.is_empty() {
        score += 10;
        reasons.push("+10 subject preenchido".to_string());
    }
    if !cert.issuer.is_empty() {
        score += 5;
        reasons.push("+5 issuer preenchido".to_string());
    }
    if cert.valid_now {
        score += 220;
        reasons.push("+220 certificado valido agora".to_string());
    } else {
        score -= 260;
        reasons.push("-260 certificado fora da validade".to_string());
    }
    if cert.supports_digital_signature {
        score += 170;
        reasons.push("+170 key usage permite assinatura digital".to_string());
    } else {
        score -= 220;
        reasons.push("-220 key usage sem assinatura digital".to_string());
    }
    if cert.is_hardware_token {
        score += 900;
        reasons.push("+900 certificado detectado como token/smart card".to_string());
    } else {
        score -= 380;
        reasons.push("-380 nao parece certificado de token/smart card".to_string());
    }
    if contains_any(
        &provider_lc,
        &[
            "software key storage provider",
            "software cryptographic provider",
        ],
    ) {
        score -= 500;
        reasons.push("-500 provider de software (sem token)".to_string());
    }
    if contains_any(
        &provider_lc,
        &[
            "smart card",
            "token",
            "safenet",
            "watchdata",
            "entersafe",
            "epass",
        ],
    ) {
        score += 280;
        reasons.push("+280 provider indica token/smart card".to_string());
    }
    if contains_any(
        &container_lc,
        &[
            "smart",
            "token",
            "safenet",
            "watchdata",
            "etoken",
            "entersafe",
            "epass",
        ],
    ) {
        score += 130;
        reasons.push("+130 container indica token/smart card".to_string());
    }
    if looks_like_guid(&cert.subject) {
        score -= 120;
        reasons.push("-120 subject parece GUID".to_string());
    }
    if looks_like_guid(&cert.issuer) {
        score -= 80;
        reasons.push("-80 issuer parece GUID".to_string());
    }
    if subject_lc.contains("localhost") || issuer_lc.contains("localhost") {
        score -= 200;
        reasons.push("-200 certificado localhost".to_string());
    }
    if subject_digit_count(&cert.subject) >= 11 {
        score += 180;
        reasons.push("+180 subject com identificador numerico (ex.: CPF/CNPJ)".to_string());
    }
    if contains_any(
        &issuer_lc,
        &[
            "icp-brasil",
            "ac ",
            "soluti",
            "certisign",
            "valid",
            "serasa",
            "serpro",
        ],
    ) {
        score += 80;
        reasons.push("+80 issuer conhecido de AC".to_string());
    }
    if contains_any(&subject_lc, &["token", "a3", "assinatura", "cpf", "cnpj"]) {
        score += 40;
        reasons.push("+40 subject com indicio de certificado de assinatura".to_string());
    }
    if !looks_like_guid(&cert.subject) && !subject_lc.contains("localhost") {
        score += 30;
        reasons.push("+30 subject com formato humano".to_string());
    }
    (score, reasons)
}

fn contains_any(s: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| s.contains(needle))
}

fn subject_digit_count(s: &str) -> usize {
    s.chars().filter(|c| c.is_ascii_digit()).count()
}

fn looks_like_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let expected = [8, 4, 4, 4, 12];
    if parts.len() != expected.len() {
        return false;
    }
    parts
        .iter()
        .zip(expected.iter())
        .all(|(part, len)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_test_certificate(cert: &OwnedCert) -> bool {
    let subject_lc = cert.subject.to_ascii_lowercase();
    let issuer_lc = cert.issuer.to_ascii_lowercase();
    (looks_like_guid(&cert.subject) && looks_like_guid(&cert.issuer))
        || subject_lc.contains("localhost")
        || issuer_lc.contains("localhost")
}

fn ensure_mode_allows_certificate(cert: &OwnedCert, mode: &str) -> Result<()> {
    if mode == "token_only" && !cert.is_hardware_token {
        bail!(
            "cert_override.mode=token_only exige certificado de token/smart card. \
             O certificado selecionado nao parece hardware token."
        );
    }
    Ok(())
}

fn log_certificate_selection(certs: &[OwnedCert], selected_idx: usize, source: &str) {
    let cert = &certs[selected_idx];
    logger::info(format!(
        "Certificado selecionado ({source}): idx={}/{}; subject='{}'; issuer='{}'; thumbprint={}; token_hardware={}; provider='{}'; container='{}'; key_spec={}; prov_type={}",
        selected_idx + 1,
        certs.len(),
        cert.subject,
        cert.issuer,
        cert.thumbprint,
        cert.is_hardware_token,
        cert.key_provider_name,
        cert.key_container_name,
        cert.key_spec,
        cert.key_provider_type
    ));
}

fn show_summary(pdfs: &[PathBuf], report: &SignReport) {
    let mut resumo = format!(
        "Assinados com sucesso: {}/{}\n",
        report.signed.len(),
        pdfs.len()
    );

    if !report.signed.is_empty() {
        resumo.push_str("\nArquivos assinados:");
        for nome in &report.signed {
            let stem = Path::new(nome)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            resumo.push_str(&format!("\n  - {nome} -> {stem}_assinado.pdf"));
        }
    }

    if !report.errors.is_empty() {
        resumo.push_str(&format!("\n\nErros ({}):", report.errors.len()));
        for err in &report.errors {
            resumo.push_str(&format!("\n  - {err}"));
        }
    }

    let (title, level) = if report.errors.is_empty() {
        ("Sucesso!", MessageLevel::Info)
    } else {
        ("Concluido com erros", MessageLevel::Warning)
    };

    MessageDialog::new()
        .set_title(title)
        .set_description(&resumo)
        .set_level(level)
        .set_buttons(MessageButtons::Ok)
        .show();
}

fn list_my_certificates() -> Result<Vec<OwnedCert>> {
    unsafe {
        let store = CertOpenSystemStoreW(HCRYPTPROV_LEGACY(0), w!("MY"))
            .context("CertOpenSystemStoreW(\"MY\") falhou")?;

        let mut results = Vec::new();
        let mut prev: Option<*const CERT_CONTEXT> = None;

        loop {
            let ctx = CertEnumCertificatesInStore(store, prev);
            if ctx.is_null() {
                break;
            }
            prev = Some(ctx as *const CERT_CONTEXT);

            if !cert_has_private_key(ctx) {
                continue;
            }

            let subject = cert_name_str(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0);
            let issuer = cert_name_str(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_NAME_ISSUER_FLAG);
            let thumbprint = cert_thumbprint_sha1(ctx)?;
            let valid_now = cert_is_valid_now(ctx);
            let supports_digital_signature = cert_supports_digital_signature(ctx);
            let key_info = cert_key_provider_info(ctx).unwrap_or_default();
            let is_hardware_token =
                looks_like_hardware_token(&key_info.provider_name, &key_info.container_name);

            let owned_ctx = CertDuplicateCertificateContext(Some(ctx as *const CERT_CONTEXT));
            if owned_ctx.is_null() {
                continue;
            }

            results.push(OwnedCert {
                subject,
                issuer,
                thumbprint,
                context: owned_ctx,
                valid_now,
                supports_digital_signature,
                key_provider_name: key_info.provider_name,
                key_container_name: key_info.container_name,
                key_provider_type: key_info.provider_type,
                key_spec: key_info.key_spec,
                is_hardware_token,
            });
        }

        let _ = CertCloseStore(store, 0);
        Ok(results)
    }
}

fn cert_has_private_key(ctx: *const CERT_CONTEXT) -> bool {
    unsafe {
        let mut size = 0u32;
        CertGetCertificateContextProperty(ctx, CERT_KEY_PROV_INFO_PROP_ID, None, &mut size).is_ok()
    }
}

#[derive(Default)]
struct CertKeyProviderInfo {
    provider_name: String,
    container_name: String,
    provider_type: u32,
    key_spec: u32,
}

fn cert_key_provider_info(ctx: *const CERT_CONTEXT) -> Result<CertKeyProviderInfo> {
    unsafe {
        let mut size = 0u32;
        CertGetCertificateContextProperty(ctx, CERT_KEY_PROV_INFO_PROP_ID, None, &mut size)
            .context("Falha ao consultar tamanho de CERT_KEY_PROV_INFO")?;

        if size == 0 {
            return Ok(CertKeyProviderInfo::default());
        }

        let mut buffer = vec![0u8; size as usize];
        CertGetCertificateContextProperty(
            ctx,
            CERT_KEY_PROV_INFO_PROP_ID,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut size,
        )
        .context("Falha ao ler CERT_KEY_PROV_INFO")?;

        let info = &*(buffer.as_ptr() as *const CRYPT_KEY_PROV_INFO);
        let provider_name = wide_ptr_to_string(info.pwszProvName.0);
        let container_name = wide_ptr_to_string(info.pwszContainerName.0);

        Ok(CertKeyProviderInfo {
            provider_name,
            container_name,
            provider_type: info.dwProvType,
            key_spec: info.dwKeySpec,
        })
    }
}

fn cert_is_valid_now(ctx: *const CERT_CONTEXT) -> bool {
    unsafe { CertVerifyTimeValidity(None, (*ctx).pCertInfo) == 0 }
}

fn cert_supports_digital_signature(ctx: *const CERT_CONTEXT) -> bool {
    const ENCODING: u32 = X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0;
    unsafe {
        let mut key_usage = [0u8; 2];
        if CertGetIntendedKeyUsage(
            CERT_QUERY_ENCODING_TYPE(ENCODING),
            (*ctx).pCertInfo,
            &mut key_usage,
        )
        .is_ok()
        {
            (key_usage[0] & CERT_DIGITAL_SIGNATURE_KEY_USAGE as u8) != 0
        } else {
            true
        }
    }
}

fn cert_thumbprint_sha1(ctx: *const CERT_CONTEXT) -> Result<String> {
    unsafe {
        let mut size = 0u32;
        CertGetCertificateContextProperty(ctx, CERT_SHA1_HASH_PROP_ID, None, &mut size)
            .context("Falha ao consultar tamanho do thumbprint")?;
        let mut buffer = vec![0u8; size as usize];
        CertGetCertificateContextProperty(
            ctx,
            CERT_SHA1_HASH_PROP_ID,
            Some(buffer.as_mut_ptr() as *mut _),
            &mut size,
        )
        .context("Falha ao ler thumbprint")?;
        buffer.truncate(size as usize);
        Ok(buffer.iter().map(|b| format!("{b:02X}")).collect())
    }
}

fn looks_like_hardware_token(provider_name: &str, container_name: &str) -> bool {
    let provider_lc = provider_name.to_ascii_lowercase();
    let container_lc = container_name.to_ascii_lowercase();
    let combined = format!("{provider_lc} {container_lc}");

    if contains_any(
        &combined,
        &[
            "smart card",
            "smartcard",
            "token",
            "etoken",
            "safenet",
            "watchdata",
            "aladdin",
            "gemalto",
            "entersafe",
            "epass",
            "pkcs11",
            "pkcs#11",
            "a3",
        ],
    ) {
        return true;
    }

    false
}

unsafe fn wide_ptr_to_string(ptr: *mut u16) -> String { unsafe {
    if ptr.is_null() {
        return String::new();
    }

    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }

    if len == 0 {
        return String::new();
    }

    let slice = slice::from_raw_parts(ptr, len);
    String::from_utf16_lossy(slice)
}}

unsafe fn cert_name_str(ctx: *const CERT_CONTEXT, name_type: u32, flags: u32) -> String { unsafe {
    let len = CertGetNameStringW(ctx, name_type, flags, None, None);
    if len <= 1 {
        return String::new();
    }

    let mut buf = vec![0u16; len as usize];
    let _ = CertGetNameStringW(ctx, name_type, flags, None, Some(&mut buf));

    String::from_utf16_lossy(&buf[..buf.len().saturating_sub(1)])
}}

static SHA256_OID: &[u8] = b"2.16.840.1.101.3.4.2.1\0";

unsafe fn cms_sign_detached(cert_ctx: *const CERT_CONTEXT, signed_bytes: &[u8]) -> Result<Vec<u8>> { unsafe {
    const ENCODING: u32 = X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0;

    let hash_alg = CRYPT_ALGORITHM_IDENTIFIER {
        pszObjId: PSTR(SHA256_OID.as_ptr() as *mut u8),
        Parameters: CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        },
    };

    let sign_para = CRYPT_SIGN_MESSAGE_PARA {
        cbSize: mem::size_of::<CRYPT_SIGN_MESSAGE_PARA>() as u32,
        dwMsgEncodingType: ENCODING,
        pSigningCert: cert_ctx,
        HashAlgorithm: hash_alg,
        pvHashAuxInfo: std::ptr::null_mut(),
        cMsgCert: 0,
        rgpMsgCert: std::ptr::null_mut(),
        cMsgCrl: 0,
        rgpMsgCrl: std::ptr::null_mut(),
        cAuthAttr: 0,
        rgAuthAttr: std::ptr::null_mut(),
        cUnauthAttr: 0,
        rgUnauthAttr: std::ptr::null_mut(),
        dwFlags: 0,
        dwInnerContentType: 0,
    };

    let data_ptr: *const u8 = signed_bytes.as_ptr();
    let data_ptrs = [data_ptr];
    let data_size: u32 = signed_bytes.len() as u32;

    let mut sig_len: u32 = 0;
    CryptSignMessage(
        &sign_para,
        BOOL(1),
        1,
        Some(data_ptrs.as_ptr()),
        &data_size,
        None,
        &mut sig_len,
    )
    .context(
        "CryptSignMessage (calcular tamanho) falhou - verifique PIN e conectividade do token",
    )?;

    let mut sig = vec![0u8; sig_len as usize];
    CryptSignMessage(
        &sign_para,
        BOOL(1),
        1,
        Some(data_ptrs.as_ptr()),
        &data_size,
        Some(sig.as_mut_ptr()),
        &mut sig_len,
    )
    .context("CryptSignMessage (assinar) falhou")?;

    sig.truncate(sig_len as usize);
    Ok(sig)
}}

fn file_name(p: &Path) -> String {
    p.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn output_name(input: &Path) -> PathBuf {
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    input.with_file_name(format!("{stem}_assinado.pdf"))
}

fn select_pdfs() -> Vec<PathBuf> {
    let desktop = env::var("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Desktop"))
        .unwrap_or_else(|_| PathBuf::from("."));
    FileDialog::new()
        .set_title("Selecione os PDFs para assinar")
        .add_filter("Arquivos PDF", &["pdf"])
        .set_directory(&desktop)
        .pick_files()
        .unwrap_or_default()
}

fn next_free_obj_num(pdf: &[u8]) -> Result<u32> {
    let txt = String::from_utf8_lossy(pdf);
    let mut max = 0u32;
    for line in txt.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[2] == "obj" {
            if let (Ok(n), Ok(0u32)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                max = max.max(n);
            }
        }
    }
    if max == 0 {
        bail!("Nao foi possivel determinar o maior numero de objeto do PDF");
    }
    Ok(max + 1)
}

fn find_contents_hex_start(pdf: &[u8], from: usize) -> Option<usize> {
    const NEEDLE: &[u8] = b"/Contents <";
    find_subsequence(&pdf[from..], NEEDLE).map(|p| from + p + NEEDLE.len() - 1)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_top_left_horizontal_rect() {
        let rect = compute_visible_signature_rect(
            [0.0, 0.0, 612.0, 792.0],
            VisibleSignaturePlacement::TopLeftHorizontal,
        );
        assert_eq!(rect, [24.0, 696.0, 244.0, 768.0]);
    }

    #[test]
    fn computes_bottom_right_vertical_rect() {
        let rect = compute_visible_signature_rect(
            [0.0, 0.0, 612.0, 792.0],
            VisibleSignaturePlacement::BottomRightVertical,
        );
        assert_eq!(rect, [478.0, 24.0, 588.0, 204.0]);
    }
}
