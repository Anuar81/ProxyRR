# 0006 — Certificate setup · Tareas

- [x] T1. `LocalSite` en el motor + enrutado de `proxyrr.cert` y `/cert` (CA 11–12). Tests: `local.rs::routes_only_the_local_site`, `http_proxy.rs::local_site_is_served_by_the_proxy_and_captured`.
- [x] T2. `CaFiles` + `.mobileconfig` (CA 7, 13). Tests: `identifiers`, `mobileconfig_embeds_the_der_and_is_stable`, `common_name_in_any_order`, `escapes_xml`.
- [x] T3. `CertSite` (CA 11, 13). Tests: `serves_every_format`, `page_adapts_to_the_device_and_the_entry_point`, `platforms`.
- [x] T4. Planes de almacén por SO + `status` + aviso de Firefox (CA 1–5). Tests: `windows_installs_for_the_user_and_removes_by_thumbprint`, `macos_uses_the_login_keychain_with_ssl_trust`, `linux_system_store_needs_sudo_and_nss_does_not`, `status_per_os`, `linux_status_finds_the_ca_inside_a_bundle`, `apply_stops_at_the_first_failure`.
- [x] T5. Guías, IP de LAN y QR (CA 6–10, 12). Tests: `remote_guides_use_the_lan_address_and_a_qr`, `ios_includes_the_trust_step`, `android_includes_network_security_config`, `every_target_has_steps_and_the_key_warning`, `lan_ip_is_never_loopback`, `qr_renders_blocks`.
- [x] T6. CLI `ca install|uninstall|status`, `setup`, página en `start` (CA 1–3, 5, 6, 8, 11). Tests en `crates/proxyrr-cli/tests/setup.rs` (7).
- [ ] T7. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada` (queda CA 9 en 0002 y CA 14 en `desktop-mvp`).
