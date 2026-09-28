# 0005 — HTTPS MITM · Tareas

- [x] T1. `HostPattern` + `Prefixed`. Tests unitarios en `mitm.rs`: `exact_pattern_matches_only_that_host`, `wildcard_pattern_matches_subdomains_only`, `ip_patterns_ignore_brackets`, `prefixed_replays_prefix_then_inner`, `prefixed_writes_go_to_inner` (CA 5, 6).
- [x] T2. `ProxyConfig { mitm, upstream_roots }` + cliente upstream HTTPS con raíces del SO. Test: `untrusted_upstream_is_502` (CA 3).
- [x] T3. Intercepción: primer byte, `LazyConfigAcceptor`, hoja por SNI / host. Tests: `mitm_decrypts_and_forwards` (keep-alive + ALPN), `ip_target_without_sni_gets_ip_leaf`, `mitm::tests::issues_server_config_per_host` (CA 1, 2, 4, 8).
- [x] T4. Bypass y no-TLS. Tests: `bypassed_host_is_not_decrypted`, `non_tls_over_connect_is_tunneled`, `server_first_protocol_is_tunneled_after_wait` (CA 5, 6).
- [x] T5. Error de confianza del cliente. Test: `client_rejecting_ca_reports_tls_error` (CA 7).
- [x] T6. CLI `start --mitm [--bypass]`. Tests: `proxyrr-cli/tests/start.rs::mitm_uses_ca_from_data_dir_and_shows_fingerprint`, `bypass_requires_mitm`, `main::tests::hides_successful_intercepted_tunnels_only` (CA 10). CA 9: `tests/http_proxy.rs` (0004) sigue verde sin cambios de comportamiento.
- [ ] T7. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
