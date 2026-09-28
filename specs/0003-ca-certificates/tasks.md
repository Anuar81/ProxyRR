# 0003 — CA certificates · Tareas

- [x] T1. Generar CA + guardar/cargar. Tests: `ca::tests::generated_ca_has_ca_constraints` (CA 1), `load_or_create_is_stable` + `loaded_ca_can_still_issue` (CA 2), `names_are_unique` (CA 3), `key_file_is_private` (CA 4, solo Unix), `corrupt_files_are_not_overwritten` + `half_present_ca_is_incomplete` + `mismatched_key_is_rejected` + `save_never_overwrites` (CA 5).
- [x] T2. Emisión de hojas. Tests: `leaf::tests::leaf_is_signed_by_ca`, `leaf_has_expected_extensions`, `ip_host_uses_ip_san`, `wildcard_host_is_accepted`, `validity_within_397_days`, `serials_are_unique`, `invalid_hosts_are_rejected` (CA 6).
- [x] T3. `LeafCache`. Tests: `cache::tests::returns_same_leaf_for_same_host`, `evicts_lru`, `concurrent_access`, `invalid_host_is_not_cached` (CA 7).
- [x] T4. Export PEM/DER. Test: `ca::tests::pem_and_der_match` (CA 8).
- [x] T5. `subject_hash_old`. Tests: `hash::tests::matches_openssl_vector` (fixture generada con OpenSSL), `tests/openssl.rs::matches_openssl_binary` (se saltea si no hay `openssl`) (CA 9).
- [x] T6. CLI `ca info|export|path` + `--data-dir`. Tests: `proxyrr-cli/tests/ca.rs` (CA 10).
- [ ] T7. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
