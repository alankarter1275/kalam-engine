//! EPUB font de-obfuscation (IDPF and Adobe schemes).
//!
//! `META-INF/encryption.xml` lists container entries whose leading bytes are
//! XOR-"encrypted" against a key derived from the package unique identifier.
//! Both schemes are obfuscation, not encryption — the spec's goal is only to
//! stop trivial font extraction. Applied transparently by `Book::resource`
//! and friends, so consumers never see obfuscated bytes.

use std::collections::HashMap;

use quick_xml::events::Event;
use sha1::{Digest, Sha1};

pub(crate) const IDPF_ALGORITHM: &str = "http://www.idpf.org/2008/embedding";
pub(crate) const ADOBE_ALGORITHM: &str = "http://ns.adobe.com/pdf/enc#RC";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Obfuscation {
    Idpf,
    Adobe,
}

/// Parse `META-INF/encryption.xml`, returning container-root paths (no
/// leading slash, percent-decoded as written) → obfuscation scheme. Entries
/// with unknown algorithms (real DRM) are ignored here; reading them will
/// simply yield garbage bytes downstream, which is the best we can do.
pub(crate) fn parse_encryption_xml(xml: &[u8]) -> HashMap<String, Obfuscation> {
    let mut out = HashMap::new();
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut current_algo: Option<Obfuscation> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = e.local_name();
                let local = local.as_ref();
                if local == "EncryptionMethod" {
                    current_algo = e
                        .try_get_attribute("Algorithm")
                        .ok()
                        .flatten()
                        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
                        .and_then(|v| match v.as_ref() {
                            IDPF_ALGORITHM => Some(Obfuscation::Idpf),
                            ADOBE_ALGORITHM => Some(Obfuscation::Adobe),
                            _ => None,
                        });
                } else if local == "CipherReference" {
                    if let Some(algo) = current_algo {
                        if let Some(uri) = e.try_get_attribute("URI").ok().flatten().and_then(|a| {
                            a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok()
                        }) {
                            out.insert(uri.trim_start_matches('/').to_string(), algo);
                        }
                    }
                } else if local == "EncryptedData" {
                    current_algo = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// XOR the obfuscated prefix of `data` in place.
pub(crate) fn deobfuscate(algo: Obfuscation, identifier: &str, data: &mut [u8]) {
    match algo {
        Obfuscation::Idpf => {
            // Key: SHA-1 of the unique identifier with whitespace removed.
            let cleaned: String = identifier
                .chars()
                .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
                .collect();
            let key = Sha1::digest(cleaned.as_bytes());
            xor_prefix(data, &key, 1040);
        }
        Obfuscation::Adobe => {
            // Key: the 16 UUID bytes of the identifier (urn:uuid: prefix and
            // hyphens stripped, hex-decoded).
            let hex: String = identifier
                .trim()
                .strip_prefix("urn:uuid:")
                .unwrap_or(identifier.trim())
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect();
            let mut key = Vec::with_capacity(16);
            let bytes = hex.as_bytes();
            let mut i = 0;
            while i + 1 < bytes.len() && key.len() < 16 {
                let hi = (bytes[i] as char).to_digit(16).unwrap_or(0) as u8;
                let lo = (bytes[i + 1] as char).to_digit(16).unwrap_or(0) as u8;
                key.push(hi << 4 | lo);
                i += 2;
            }
            if key.len() == 16 {
                xor_prefix(data, &key, 1024);
            }
        }
    }
}

fn xor_prefix(data: &mut [u8], key: &[u8], prefix_len: usize) {
    for (i, byte) in data.iter_mut().take(prefix_len).enumerate() {
        *byte ^= key[i % key.len()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_encryption_entries() {
        let xml = br#"<?xml version="1.0"?>
<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"
            xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/fonts/serif.otf"/></enc:CipherData>
  </enc:EncryptedData>
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://ns.adobe.com/pdf/enc#RC"/>
    <enc:CipherData><enc:CipherReference URI="/OEBPS/fonts/sans.ttf"/></enc:CipherData>
  </enc:EncryptedData>
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes256-cbc"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/secret.bin"/></enc:CipherData>
  </enc:EncryptedData>
</encryption>"#;
        let map = parse_encryption_xml(xml);
        assert_eq!(map.get("OEBPS/fonts/serif.otf"), Some(&Obfuscation::Idpf));
        assert_eq!(map.get("OEBPS/fonts/sans.ttf"), Some(&Obfuscation::Adobe));
        assert!(
            !map.contains_key("OEBPS/secret.bin"),
            "real DRM must not be claimed"
        );
    }

    #[test]
    fn idpf_roundtrip() {
        let identifier = "urn:uuid:3c5b7a02-1f10-4e3c-9e40-000000000001";
        let original: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        let mut data = original.clone();
        deobfuscate(Obfuscation::Idpf, identifier, &mut data);
        assert_ne!(data, original, "prefix must change");
        assert_eq!(
            data[1040..],
            original[1040..],
            "only the 1040-byte prefix is touched"
        );
        deobfuscate(Obfuscation::Idpf, identifier, &mut data);
        assert_eq!(data, original, "XOR is its own inverse");
    }

    #[test]
    fn adobe_roundtrip_and_key_parsing() {
        let identifier = "urn:uuid:12345678-9abc-def0-1234-56789abcdef0";
        let original: Vec<u8> = (0..1500u32).map(|i| (i % 253) as u8).collect();
        let mut data = original.clone();
        deobfuscate(Obfuscation::Adobe, identifier, &mut data);
        assert_ne!(data, original);
        assert_eq!(data[1024..], original[1024..]);
        deobfuscate(Obfuscation::Adobe, identifier, &mut data);
        assert_eq!(data, original);
    }

    #[test]
    fn short_data_is_safe() {
        let mut data = vec![1u8, 2, 3];
        deobfuscate(Obfuscation::Idpf, "urn:uuid:x", &mut data);
        deobfuscate(Obfuscation::Idpf, "urn:uuid:x", &mut data);
        assert_eq!(data, vec![1, 2, 3]);
    }
}
