//! Caché LRU de certificados hoja, segura entre hilos.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lru::LruCache;

use crate::leaf::{LeafCert, normalize_host};
use crate::{CertificateAuthority, Result};

/// Capacidad por defecto: sobra para una sesión de depuración normal.
const DEFAULT_CAPACITY: NonZeroUsize = NonZeroUsize::new(1024).unwrap();

/// Caché de hojas por host. Emitir una hoja cuesta ~1 ms; servirla de caché, nada.
pub struct LeafCache {
    inner: Mutex<LruCache<String, Arc<LeafCert>>>,
}

impl fmt::Debug for LeafCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.lock();
        f.debug_struct("LeafCache")
            .field("len", &guard.len())
            .field("capacity", &guard.cap())
            .finish()
    }
}

impl Default for LeafCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl LeafCache {
    /// Caché con capacidad máxima `capacity`.
    #[must_use]
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// Devuelve la hoja cacheada para `host` o la emite con `ca` y la guarda.
    ///
    /// La emisión ocurre fuera del lock, así hosts distintos no se bloquean entre sí.
    /// Si dos hilos emiten a la vez para el mismo host, gana el primero en guardar y
    /// ambos reciben esa misma hoja.
    ///
    /// # Errors
    /// Si el host es inválido o falla la emisión.
    pub fn get_or_issue(&self, ca: &CertificateAuthority, host: &str) -> Result<Arc<LeafCert>> {
        let key = normalize_host(host)?;
        if let Some(hit) = self.lock().get(&key) {
            return Ok(Arc::clone(hit));
        }
        let fresh = Arc::new(ca.issue_leaf(&key)?);
        let mut guard = self.lock();
        if let Some(existing) = guard.get(&key) {
            return Ok(Arc::clone(existing));
        }
        guard.put(key, Arc::clone(&fresh));
        Ok(fresh)
    }

    /// Cantidad de hojas en caché.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// `true` si la caché está vacía.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> MutexGuard<'_, LruCache<String, Arc<LeafCert>>> {
        // Un pánico con el lock tomado no deja la caché inconsistente: se sigue usando.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn returns_same_leaf_for_same_host() {
        let ca = CertificateAuthority::generate().unwrap();
        let cache = LeafCache::default();
        let a = cache.get_or_issue(&ca, "example.com").unwrap();
        let b = cache.get_or_issue(&ca, "EXAMPLE.com").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn evicts_lru() {
        let ca = CertificateAuthority::generate().unwrap();
        let cache = LeafCache::new(NonZeroUsize::new(2).unwrap());
        let a1 = cache.get_or_issue(&ca, "a.com").unwrap();
        cache.get_or_issue(&ca, "b.com").unwrap();
        cache.get_or_issue(&ca, "c.com").unwrap(); // expulsa a.com
        assert_eq!(cache.len(), 2);
        let a2 = cache.get_or_issue(&ca, "a.com").unwrap();
        assert!(!Arc::ptr_eq(&a1, &a2));
    }

    #[test]
    fn concurrent_access() {
        let ca = CertificateAuthority::generate().unwrap();
        let cache = LeafCache::default();
        let leaves: Vec<Arc<LeafCert>> = thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| cache.get_or_issue(&ca, "same.host").unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(leaves.windows(2).all(|w| Arc::ptr_eq(&w[0], &w[1])));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn invalid_host_is_not_cached() {
        let ca = CertificateAuthority::generate().unwrap();
        let cache = LeafCache::default();
        assert!(cache.get_or_issue(&ca, "bad host").is_err());
        assert!(cache.is_empty());
    }
}
