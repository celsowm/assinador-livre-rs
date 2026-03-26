#[cfg(windows)]
use crate::{config::CertOverride, logger};
#[cfg(windows)]
use anyhow::{Context, Result};
#[cfg(windows)]
use windows::{
    Win32::{Foundation::BOOL, Security::Cryptography::*},
    core::{PSTR, w},
};
#[cfg(windows)]
use std::mem;
#[cfg(windows)]
use std::slice;

#[cfg(windows)]
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

#[cfg(windows)]
unsafe impl Send for OwnedCert {}
#[cfg(windows)]
unsafe impl Sync for OwnedCert {}

#[cfg(windows)]
impl Drop for OwnedCert {
    fn drop(&mut self) {
        unsafe {
            let _ = CertFreeCertificateContext(Some(self.context));
        }
    }
}

#[cfg(windows)]
pub fn list_my_certificates() -> Result<Vec<OwnedCert>> {
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

#[cfg(windows)]
fn cert_has_private_key(ctx: *const CERT_CONTEXT) -> bool {
    unsafe {
        let mut size = 0u32;
        CertGetCertificateContextProperty(ctx, CERT_KEY_PROV_INFO_PROP_ID, None, &mut size).is_ok()
    }
}

#[cfg(windows)]
#[derive(Default)]
struct CertKeyProviderInfo {
    provider_name: String,
    container_name: String,
    provider_type: u32,
    key_spec: u32,
}

#[cfg(windows)]
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

#[cfg(windows)]
fn cert_is_valid_now(ctx: *const CERT_CONTEXT) -> bool {
    unsafe { CertVerifyTimeValidity(None, (*ctx).pCertInfo) == 0 }
}

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
fn looks_like_hardware_token(provider_name: &str, container_name: &str) -> bool {
    let provider_lc = provider_name.to_ascii_lowercase();
    let container_lc = container_name.to_ascii_lowercase();
    let combined = format!("{provider_lc} {container_lc}");

    if super::contains_any(
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

#[cfg(windows)]
unsafe fn wide_ptr_to_string(ptr: *mut u16) -> String {
    unsafe {
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
    }
}

#[cfg(windows)]
unsafe fn cert_name_str(ctx: *const CERT_CONTEXT, name_type: u32, flags: u32) -> String {
    unsafe {
        let len = CertGetNameStringW(ctx, name_type, flags, None, None);
        if len <= 1 {
            return String::new();
        }

        let mut buf = vec![0u16; len as usize];
        let _ = CertGetNameStringW(ctx, name_type, flags, None, Some(&mut buf));

        String::from_utf16_lossy(&buf[..buf.len().saturating_sub(1)])
    }
}

#[cfg(windows)]
static SHA256_OID: &[u8] = b"2.16.840.1.101.3.4.2.1\0";

#[cfg(windows)]
pub unsafe fn cms_sign_detached(cert_ctx: *const CERT_CONTEXT, signed_bytes: &[u8]) -> Result<Vec<u8>> {
    unsafe {
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
}
