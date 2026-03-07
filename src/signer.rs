use crate::config::CertOverride;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use lopdf::Document;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::{
    env, fs,
    io::{self, Write as _},
    mem,
    path::{Path, PathBuf},
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
}

pub struct OwnedCert {
    pub subject: String,
    pub issuer: String,
    pub thumbprint: String,
    pub context: *const CERT_CONTEXT,
    pub valid_now: bool,
    pub supports_digital_signature: bool,
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
) -> Result<WsSignResult> {
    let certs = load_available_certificates()?;
    let cert_idx = choose_certificate_index(&certs, cert_override, verbose)?;
    let cert = &certs[cert_idx];

    let signed_pdf = sign_pdf_bytes(input, cert.context)?;

    Ok(WsSignResult {
        signed_pdf,
        cert_subject: cert.subject.clone(),
        cert_issuer: cert.issuer.clone(),
    })
}

pub fn sign_pdf_file(input: &Path, output: &Path, cert_ctx: *const CERT_CONTEXT) -> Result<()> {
    let original = fs::read(input)?;
    let signed = sign_pdf_bytes(&original, cert_ctx)
        .with_context(|| format!("Falha ao assinar {}", input.display()))?;
    fs::write(output, signed).with_context(|| format!("Falha ao gravar {}", output.display()))?;
    Ok(())
}

pub fn sign_pdf_bytes(input: &[u8], cert_ctx: *const CERT_CONTEXT) -> Result<Vec<u8>> {
    let original = input.to_vec();
    let doc = Document::load_mem(input).context("Falha ao abrir PDF em memoria")?;

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

    let (pages_num, pages_gen) = catalog
        .get(b"Pages")
        .context("Catalog sem /Pages")?
        .as_reference()
        .context("/Pages nao e referencia")?;

    let existing_fields: Vec<String> = catalog
        .get(b"AcroForm")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_dict().ok())
        .and_then(|d| d.get(b"Fields").ok())
        .and_then(|f| f.as_array().ok())
        .map(|arr| {
            arr.iter()
                .filter_map(|o| o.as_reference().ok())
                .map(|(n, g)| format!("{n} {g} R"))
                .collect()
        })
        .unwrap_or_default();

    const SIG_BYTES: usize = 12_288;
    const HEX_LEN: usize = SIG_BYTES * 2;
    const BR_PLACEHOLDER: &[u8] = b"/ByteRange [0 AAAAAAAAAA BBBBBBBBBB CCCCCCCCCC]";

    let next_obj = next_free_obj_num(&original)?;
    let sig_num = next_obj;
    let fld_num = next_obj + 1;
    let af_num = next_obj + 2;

    let mut upd = Vec::<u8>::new();

    let sig_off = original.len() + upd.len();
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
    write!(upd, "/M (D:{})\n", Utc::now().format("%Y%m%d%H%M%S+00'00'"))?;
    write!(upd, ">>\nendobj\n")?;

    let fld_off = original.len() + upd.len();
    write!(upd, "{fld_num} 0 obj\n<<\n")?;
    write!(upd, "/Type /Annot\n/Subtype /Widget\n/FT /Sig\n")?;
    write!(upd, "/T (Assinatura_Digital_A3)\n")?;
    write!(upd, "/V {sig_num} 0 R\n")?;
    write!(upd, "/Rect [0 0 0 0]\n")?;
    write!(upd, "/P {pages_num} {pages_gen} R\n")?;
    write!(upd, ">>\nendobj\n")?;

    let af_off = original.len() + upd.len();
    let all_fields = {
        let mut v = existing_fields.clone();
        v.push(format!("{fld_num} 0 R"));
        v.join(" ")
    };
    write!(upd, "{af_num} 0 obj\n<<\n")?;
    write!(upd, "/Fields [{all_fields}]\n/SigFlags 3\n")?;
    write!(upd, ">>\nendobj\n")?;

    let cat_off = original.len() + upd.len();
    write!(upd, "{cat_num} {cat_gen} obj\n<<\n")?;
    write!(upd, "/Type /Catalog\n")?;
    write!(upd, "/Pages {pages_num} {pages_gen} R\n")?;
    write!(upd, "/AcroForm {af_num} 0 R\n")?;
    write!(upd, ">>\nendobj\n")?;

    let xref_off = original.len() + upd.len();
    write!(upd, "\nxref\n")?;
    write!(upd, "{next_obj} 3\n")?;
    write!(upd, "{sig_off:010} 00000 n \n")?;
    write!(upd, "{fld_off:010} 00000 n \n")?;
    write!(upd, "{af_off:010} 00000 n \n")?;
    write!(upd, "{cat_num} 1\n")?;
    write!(upd, "{cat_off:010} {cat_gen:05} n \n")?;
    write!(upd, "trailer\n<<\n")?;
    write!(upd, "/Size {}\n", next_obj + 3)?;
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
            return Ok(index - 1);
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
            return Ok(idx - 1);
        }
        bail!(
            "ASSINADOR_CERT_INDEX invalido: '{}'. Valores aceitos: 1..={}.",
            raw.trim(),
            certs.len()
        );
    }

    if certs.len() == 1 {
        return Ok(0);
    }

    let candidate_indexes: Vec<usize> = certs
        .iter()
        .enumerate()
        .filter_map(|(idx, cert)| (!is_test_certificate(cert)).then_some(idx))
        .collect();
    let using_filtered_set = !candidate_indexes.is_empty();

    let all_candidates: Vec<usize> = (0..certs.len()).collect();
    let ranked = rank_certificates(
        certs,
        if using_filtered_set {
            &candidate_indexes
        } else {
            &all_candidates
        },
    );
    let best_idx = ranked[0].index;

    if verbose {
        println!("[AUTO][verbose] Motivos por certificado:");
        for entry in &ranked {
            println!(
                "  [{}] score={} {}",
                entry.index + 1,
                entry.score,
                certs[entry.index].subject
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

unsafe fn cert_name_str(ctx: *const CERT_CONTEXT, name_type: u32, flags: u32) -> String {
    let len = CertGetNameStringW(ctx, name_type, flags, None, None);
    if len <= 1 {
        return String::new();
    }

    let mut buf = vec![0u16; len as usize];
    let _ = CertGetNameStringW(ctx, name_type, flags, None, Some(&mut buf));

    String::from_utf16_lossy(&buf[..buf.len().saturating_sub(1)])
}

static SHA256_OID: &[u8] = b"2.16.840.1.101.3.4.2.1\0";

unsafe fn cms_sign_detached(cert_ctx: *const CERT_CONTEXT, signed_bytes: &[u8]) -> Result<Vec<u8>> {
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
}

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
