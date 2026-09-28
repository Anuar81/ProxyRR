# 0003 — CA certificates · Diseño

## Crate `proxyrr-cert`

```
src/
  lib.rs        re-exports + Error
  ca.rs         CertificateAuthority: generar, cargar/guardar, export, info
  leaf.rs       LeafCert + emisión firmada por la CA
  cache.rs      LeafCache (LRU + Mutex)
  hash.rs       subject_hash_old
```

### Tipos públicos

```rust
pub struct CertificateAuthority { .. }          // cert DER + PEM, clave, Issuer de rcgen
impl CertificateAuthority {
    pub fn generate() -> Result<Self>;
    pub fn load_or_create(dir: &Path) -> Result<Self>;
    pub fn load(dir: &Path) -> Result<Self>;
    pub fn cert_pem(&self) -> &str;
    pub fn cert_der(&self) -> &[u8];
    pub fn info(&self) -> Result<CaInfo>;        // subject, not_after, sha256, subject_hash_old
    pub fn issue_leaf(&self, host: &str) -> Result<LeafCert>;
}
pub struct LeafCert { pub cert_der: Vec<u8>, pub key_der: Vec<u8> }   // listo para rustls
pub struct LeafCache { .. }                     // get_or_issue(&ca, host) -> Result<Arc<LeafCert>>
pub fn subject_hash_old(cert_der: &[u8]) -> Result<u32>;
```

### Decisiones

- **rcgen 0.14** (backend `ring`, feature `x509-parser` para recargar la CA desde PEM). `ring` evita
  CMake/NASM en Windows que pide `aws-lc-rs`.
- **Clave ECDSA P-256**: rápida de generar (hojas al vuelo) y aceptada por todos los clientes modernos.
- **Validez**: CA 10 años; hoja `now - 1 día .. now + 396 días` (total 397).
- **Serial** aleatorio de 16 bytes en CA y hojas (los navegadores rechazan seriales repetidos de un mismo issuer).
- **`subject_hash_old`**: MD5 del DER del `Name` del subject; los primeros 4 bytes leídos little-endian.
  El DER del subject sale de `x509-parser` (`subject().as_raw()`).
- **Escritura sin pisar**: temporal con nombre aleatorio creado con `create_new` (0600 en Unix desde su
  creación) y publicado con hard link, que es atómico y falla si el destino existe. La clave es el punto
  de commit; si dos procesos crean la CA a la vez, el que pierde carga la del ganador. Nunca se sobrescribe.
- **Caché**: `lru::LruCache<String, Arc<LeafCert>>` detrás de `Mutex`; host normalizado a minúsculas.
  La emisión ocurre fuera del lock para no serializar hosts distintos.
- **Directorio por defecto**: `%APPDATA%\ProxyRR` (Windows), `~/Library/Application Support/ProxyRR` (macOS),
  `$XDG_DATA_HOME/proxyrr` o `~/.local/share/proxyrr` (Linux). Calculado a mano: el crate `directories`
  arrastra `option-ext` (MPL-2.0) y `cargo deny` lo rechaza.

## CLI

`proxyrr [--data-dir <dir>] ca <info|export|path>`. `info` crea la CA si no existe.

## Riesgos

- Hash de Android mal calculado → la CA no aparece en el store. Mitigado con test contra valor de OpenSSL.
- La clave de la CA en disco permite interceptar el tráfico de quien la confíe: se documenta y en Unix va `0600`.
