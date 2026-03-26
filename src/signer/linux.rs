use crate::logger;
use anyhow::{Context, Result, bail};
use cryptoki::{
    context::Pkcs11,
    object::{Attribute, ObjectClass, AttributeType},
    mechanism::Mechanism,
    session::UserType,
};
use openssl::x509::{X509, X509NameRef};
use std::{env, path::{Path, PathBuf}};
use secrecy::Secret;

// CMS assembly dependencies
use cms::{
    content_info::{ContentInfo, CmsVersion},
    signed_data::{SignedData, SignerInfo, SignerIdentifier, EncapsulatedContentInfo, SignerInfos, CertificateSet},
    cert::{CertificateChoices, IssuerAndSerialNumber},
};
use der::{Decode, Encode, asn1::{SetOfVec, OctetString}};
use x509_cert::Certificate;
use x509_cert::spki::AlgorithmIdentifierOwned;

pub struct OwnedCert {
    pub subject: String,
    pub issuer: String,
    pub thumbprint: String,
    pub valid_now: bool,
    pub supports_digital_signature: bool,
    pub key_provider_name: String,
    pub key_container_name: String,
    pub key_provider_type: u32,
    pub key_spec: u32,
    pub is_hardware_token: bool,
    pub slot_id: u64,
    pub cert_id: Vec<u8>,
    pub pkcs11_lib: PathBuf,
}

pub fn list_my_certificates() -> Result<Vec<OwnedCert>> {
    let mut certs = Vec::new();
    let libs = pkcs11_libs();

    for lib_path in libs {
        if !lib_path.exists() {
            continue;
        }

        if let Err(e) = list_certs_from_lib(&lib_path, &mut certs) {
            logger::warn(format!("Erro ao ler PKCS#11 {} : {:#}", lib_path.display(), e));
        }
    }

    Ok(certs)
}

fn list_certs_from_lib(lib_path: &Path, results: &mut Vec<OwnedCert>) -> Result<()> {
    let pkcs11 = Pkcs11::new(lib_path)?;
    pkcs11.initialize(cryptoki::context::CInitializeArgs::OsThreads)?;

    let slots = pkcs11.get_slots_with_token()?;
    for slot in slots {
        let session = match pkcs11.open_ro_session(slot) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let template = [
            Attribute::Class(ObjectClass::CERTIFICATE),
        ];

        let objects = session.find_objects(&template)?;
        for obj in objects {
            let attributes = match session.get_attributes(obj, &[
                AttributeType::Subject,
                AttributeType::Issuer,
                AttributeType::Id,
                AttributeType::Value,
            ]) {
                Ok(a) => a,
                Err(_) => continue,
            };

            let mut subject = String::new();
            let mut issuer = String::new();
            let mut cert_id = Vec::new();
            let mut cert_value = Vec::new();

            for attr in attributes {
                match attr {
                    Attribute::Subject(s) => subject = parse_dn_bytes(&s),
                    Attribute::Issuer(i) => issuer = parse_dn_bytes(&i),
                    Attribute::Id(id) => cert_id = id,
                    Attribute::Value(v) => cert_value = v,
                    _ => {}
                }
            }

            if cert_value.is_empty() {
                continue;
            }

            let x509 = match X509::from_der(&cert_value) {
                Ok(x) => x,
                Err(_) => continue,
            };
            let thumbprint = match x509.digest(openssl::hash::MessageDigest::sha1()) {
                Ok(t) => hex::encode(t).to_uppercase(),
                Err(_) => continue,
            };

            if subject.is_empty() || subject.len() < 10 || is_all_hex(&subject) {
                subject = format_x509_name(x509.subject_name());
            }
            if issuer.is_empty() || issuer.len() < 10 || is_all_hex(&issuer) {
                issuer = format_x509_name(x509.issuer_name());
            }

            results.push(OwnedCert {
                subject,
                issuer,
                thumbprint,
                valid_now: true,
                supports_digital_signature: true,
                key_provider_name: lib_path.to_string_lossy().into_owned(),
                key_container_name: format!("Slot {}", slot),
                key_provider_type: 0,
                key_spec: 0,
                is_hardware_token: true,
                slot_id: slot.into(),
                cert_id,
                pkcs11_lib: lib_path.to_path_buf(),
            });
        }
    }

    Ok(())
}

fn pkcs11_libs() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    if let Ok(val) = env::var("ASSINADOR_PKCS11_LIB") {
        libs.push(PathBuf::from(val));
    }

    let common = [
        "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
        "/usr/lib/opensc-pkcs11.so",
        "/usr/lib64/opensc-pkcs11.so",
        "/usr/local/lib/opensc-pkcs11.so",
        "/usr/lib/libeTPkcs11.so",
        "/usr/lib64/libeTPkcs11.so",
        "/usr/local/lib/libeTPkcs11.so",
        "/usr/lib/libgdpkcs11.so",
        "/usr/lib64/libgdpkcs11.so",
        "/opt/GD/lib64/libgdpkcs11.so",
        "/opt/GD/lib/libgdpkcs11.so",
        "/usr/lib/libwdpkcs_ca.so",
        "/usr/lib64/libwdpkcs_ca.so",
        "/usr/lib/libidp_pkcs11.so",
        "/usr/lib64/libidp_pkcs11.so",
        "/usr/lib/libpn_pkcs11.so",
        "/usr/lib/libawppcksc11.so",
        "/opt/AWP/lib/libawppcksc11.so",
        "/opt/AWP/lib64/libawppcksc11.so",
    ];

    for path in common {
        let p = PathBuf::from(path);
        if !libs.contains(&p) {
            libs.push(p);
        }
    }

    libs
}

fn parse_dn_bytes(der: &[u8]) -> String {
    if der.is_empty() {
        return String::new();
    }
    if der.iter().all(|&b| b >= 32 && b <= 126) {
        return String::from_utf8_lossy(der).to_string();
    }
    format!("0x{}", hex::encode(der))
}

fn is_all_hex(s: &str) -> bool {
    let s = s.strip_prefix("0x").unwrap_or(s);
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn format_x509_name(name: &X509NameRef) -> String {
    let mut parts = Vec::new();
    for entry in name.entries() {
        if let Ok(s) = entry.data().as_utf8() {
            let oid = entry.object().to_string();
            let label = match oid.as_str() {
                "commonName" => "CN",
                "organizationName" => "O",
                "organizationalUnitName" => "OU",
                "countryName" => "C",
                "localityName" => "L",
                "stateOrProvinceName" => "ST",
                _ => &oid,
            };
            parts.push(format!("{}={}", label, s));
        }
    }
    if parts.is_empty() {
        return "Unknown".to_string();
    }
    parts.join(", ")
}

pub fn cms_sign_detached(cert_info: &OwnedCert, signed_bytes: &[u8], pin: Option<String>) -> Result<Vec<u8>> {
    let pkcs11 = Pkcs11::new(&cert_info.pkcs11_lib)?;
    pkcs11.initialize(cryptoki::context::CInitializeArgs::OsThreads)?;

    let slot = cert_info.slot_id.try_into()?;
    let session = pkcs11.open_rw_session(slot)?;

    if let Some(p) = pin {
        session.login(UserType::User, Some(&Secret::new(p)))
            .context("Falha ao realizar login no token (PIN incorreto?)")?;
    }

    let key_template = [
        Attribute::Class(ObjectClass::PRIVATE_KEY),
        Attribute::Id(cert_info.cert_id.clone()),
    ];
    let key_objs = session.find_objects(&key_template)?;
    let key_obj = key_objs.first().context("Chave privada nao encontrada no token")?;

    let cert_template = [
        Attribute::Class(ObjectClass::CERTIFICATE),
        Attribute::Id(cert_info.cert_id.clone()),
    ];
    let cert_objs = session.find_objects(&cert_template)?;
    let cert_obj_handle = cert_objs.first().context("Certificado nao encontrado no token")?;
    let cert_attr = session.get_attributes(*cert_obj_handle, &[AttributeType::Value])?;
    let cert_der = match cert_attr.first() {
        Some(Attribute::Value(v)) => v,
        _ => bail!("Falha ao ler valor do certificado"),
    };

    let x509_cert = Certificate::from_der(cert_der)?;

    // Hash the data
    let digest = openssl::hash::hash(openssl::hash::MessageDigest::sha256(), signed_bytes)?;

    let sha256_oid = const_oid::ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
    let rsa_encryption_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
    let data_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
    let signed_data_oid = const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");

    #[derive(der::Sequence)]
    struct DigestInfo {
        digest_algorithm: x509_cert::spki::AlgorithmIdentifier<der::Any>,
        digest: OctetString,
    }

    let digest_info = DigestInfo {
        digest_algorithm: x509_cert::spki::AlgorithmIdentifier {
            oid: sha256_oid,
            parameters: Some(der::Any::from_der(&[0x05, 0x00])?),
        },
        digest: OctetString::new(digest.to_vec())?,
    };

    let digest_info_der = digest_info.to_der()?;

    // Perform raw signature on DigestInfo
    let mechanism = Mechanism::RsaPkcs;
    let signature_value = session.sign(&mechanism, *key_obj, &digest_info_der)
        .context("Falha na operacao de assinatura PKCS#11 - verifique se o token exige login/PIN")?;

    // Assembly PKCS#7 / CMS SignedData
    let digest_algorithms = vec![AlgorithmIdentifierOwned {
        oid: sha256_oid,
        parameters: Some(der::Any::from_der(&[0x05, 0x00])?), // NULL
    }];

    let encap_content_info = EncapsulatedContentInfo {
        econtent_type: data_oid,
        econtent: None,
    };

    let ias = IssuerAndSerialNumber {
        issuer: x509_cert.tbs_certificate.issuer.clone(),
        serial_number: x509_cert.tbs_certificate.serial_number.clone(),
    };

    let signer_info = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(ias),
        digest_alg: AlgorithmIdentifierOwned {
            oid: sha256_oid,
            parameters: Some(der::Any::from_der(&[0x05, 0x00])?), // NULL
        },
        signed_attrs: None,
        signature_algorithm: AlgorithmIdentifierOwned {
            oid: rsa_encryption_oid,
            parameters: Some(der::Any::from_der(&[0x05, 0x00])?), // NULL
        },
        signature: der::asn1::OctetString::new(signature_value)?,
        unsigned_attrs: None,
    };

    let signed_data = SignedData {
        version: CmsVersion::V1,
        digest_algorithms: digest_algorithms.try_into().map_err(|_| anyhow::anyhow!("Falha ao converter digest algorithms"))?,
        encap_content_info,
        certificates: Some(CertificateSet::from(SetOfVec::from_iter(vec![CertificateChoices::Certificate(x509_cert)]).map_err(|_| anyhow::anyhow!("Falha ao criar certificate set"))?)),
        crls: None,
        signer_infos: SignerInfos::from(SetOfVec::from_iter(vec![signer_info]).map_err(|_| anyhow::anyhow!("Falha ao criar set of signer infos"))?),
    };

    let content_info = ContentInfo {
        content_type: signed_data_oid,
        content: der::Any::from_der(&signed_data.to_der()?)?,
    };

    Ok(content_info.to_der()?)
}
