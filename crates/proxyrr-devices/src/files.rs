//! La CA en cada formato que piden los sistemas: PEM, DER y perfil `.mobileconfig` de iOS.
//! Solo el certificado público: la clave nunca pasa por acá (CA 13).

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use proxyrr_cert::CertificateAuthority;
use ring::digest::{SHA1_FOR_LEGACY_USE_ONLY, SHA256, digest};

/// Certificado público de la CA con sus identificadores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaFiles {
    /// Nombre visible (el CN), p. ej. `ProxyRR CA (d1575e05)`. Único por instalación.
    pub name: String,
    /// Certificado en PEM.
    pub pem: String,
    /// Certificado en DER.
    pub der: Vec<u8>,
    /// Huella SHA-256 en hex mayúsculas con `:` (la que se muestra al usuario).
    pub sha256: String,
    /// Huella SHA-1 en hex mayúsculas sin separadores: es la que usan `certutil` (Windows) y
    /// `security` (macOS) para identificar un certificado sin ambigüedad.
    pub sha1: String,
    /// Nombre del archivo en el store de sistema de Android (`xxxxxxxx.0`).
    pub android_file: String,
}

impl CaFiles {
    /// Arma los archivos a partir de la CA.
    ///
    /// # Errors
    /// Si el certificado de la CA no se puede interpretar.
    pub fn from_ca(ca: &CertificateAuthority) -> Result<Self, proxyrr_cert::Error> {
        let info = ca.info()?;
        let der = ca.cert_der().to_vec();
        let name = common_name(&info.subject).to_owned();
        Ok(Self {
            name,
            pem: ca.cert_pem().to_owned(),
            sha256: info.sha256_fingerprint,
            sha1: hex_upper(digest(&SHA1_FOR_LEGACY_USE_ONLY, &der).as_ref()),
            android_file: format!("{:08x}.0", info.subject_hash_old),
            der,
        })
    }

    /// Perfil de configuración de iOS/iPadOS/macOS que instala la CA como raíz. Después de
    /// instalarlo en iOS hay que activar la confianza total a mano (CA 7).
    #[must_use]
    pub fn mobileconfig(&self) -> String {
        let id = self.id();
        let data = STANDARD.encode(&self.der);
        let name = xml_escape(&self.name);
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadCertificateFileName</key>
      <string>proxyrr-ca.cer</string>
      <key>PayloadContent</key>
      <data>{data}</data>
      <key>PayloadDescription</key>
      <string>Raíz de ProxyRR para depurar HTTPS.</string>
      <key>PayloadDisplayName</key>
      <string>{name}</string>
      <key>PayloadIdentifier</key>
      <string>com.proxyrr.ca.{id}</string>
      <key>PayloadType</key>
      <string>com.apple.security.root</string>
      <key>PayloadUUID</key>
      <string>{cert_uuid}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>PayloadDescription</key>
  <string>Instala la CA de ProxyRR. Quien tenga su clave puede leer tu HTTPS: quitá el perfil cuando no lo uses.</string>
  <key>PayloadDisplayName</key>
  <string>{name}</string>
  <key>PayloadIdentifier</key>
  <string>com.proxyrr.profile.{id}</string>
  <key>PayloadRemovalDisallowed</key>
  <false/>
  <key>PayloadType</key>
  <string>Configuration</string>
  <key>PayloadUUID</key>
  <string>{profile_uuid}</string>
  <key>PayloadVersion</key>
  <integer>1</integer>
</dict>
</plist>
"#,
            cert_uuid = self.uuid("cert"),
            profile_uuid = self.uuid("profile"),
        )
    }

    /// Id corto y estable de esta CA (los primeros 8 hex de la SHA-1).
    #[must_use]
    pub fn id(&self) -> String {
        self.sha1[..8].to_ascii_lowercase()
    }

    /// UUID estable derivado de la CA: reinstalar el mismo perfil lo reemplaza en vez de duplicarlo.
    fn uuid(&self, purpose: &str) -> String {
        let mut input = self.der.clone();
        input.extend_from_slice(purpose.as_bytes());
        let hash = digest(&SHA256, &input);
        let mut b: [u8; 16] = hash.as_ref()[..16]
            .try_into()
            .expect("SHA-256 tiene 32 bytes");
        b[6] = (b[6] & 0x0f) | 0x50; // versión 5 (derivado de un nombre)
        b[8] = (b[8] & 0x3f) | 0x80; // variante RFC 4122
        let h = hex_upper(&b);
        format!(
            "{}-{}-{}-{}-{}",
            &h[..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
    }
}

/// Valor del `CN=` de un subject como `CN=ProxyRR CA (x), O=ProxyRR` (en cualquier orden); si no
/// tiene CN, el subject entero.
fn common_name(subject: &str) -> &str {
    subject
        .split(", ")
        .find_map(|part| part.strip_prefix("CN="))
        .unwrap_or(subject)
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02X}");
        out
    })
}

pub(crate) fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn files() -> CaFiles {
        let ca = CertificateAuthority::generate().unwrap();
        CaFiles::from_ca(&ca).unwrap()
    }

    #[test]
    fn identifiers() {
        let f = files();
        assert!(f.name.starts_with("ProxyRR CA ("), "{}", f.name);
        assert!(
            f.name.ends_with(')') && !f.name.contains("O="),
            "solo el CN: {}",
            f.name
        );
        assert_eq!(f.sha1.len(), 40);
        assert!(
            f.sha1
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase())
        );
        assert!(f.android_file.ends_with(".0") && f.android_file.len() == 10);
        assert!(f.pem.starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(!f.pem.contains("PRIVATE"), "nunca la clave");
    }

    #[test]
    fn mobileconfig_embeds_the_der_and_is_stable() {
        let f = files();
        let profile = f.mobileconfig();
        assert!(profile.contains(&STANDARD.encode(&f.der)));
        assert!(profile.contains("<string>com.apple.security.root</string>"));
        assert!(profile.contains(&format!("<string>{}</string>", f.name)));
        assert_eq!(profile, f.mobileconfig(), "UUIDs estables");
        let uuid = f.uuid("cert");
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "5");
        assert_ne!(uuid, f.uuid("profile"));
        assert!(!profile.contains("PRIVATE"));
    }

    #[test]
    fn common_name_in_any_order() {
        assert_eq!(
            common_name("CN=ProxyRR CA (ab), O=ProxyRR"),
            "ProxyRR CA (ab)"
        );
        assert_eq!(
            common_name("O=ProxyRR, CN=ProxyRR CA (ab)"),
            "ProxyRR CA (ab)"
        );
        assert_eq!(common_name("O=Solo"), "O=Solo");
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(xml_escape(r#"a&b<c>"d""#), "a&amp;b&lt;c&gt;&quot;d&quot;");
    }
}
