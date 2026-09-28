# 0004 — HTTP proxy · Tareas

- [x] T1. `headers::strip_hop_by_hop`. Tests unitarios (CA 2).
- [x] T2. `Proxy::start` / `Proxy::subscribe` + eventos. Tests: `emits_events_with_incremental_ids`, `shutdown_releases_the_port` (CA 7).
- [x] T3. Forward HTTP. Tests: `forwards_get_in_origin_form`, `forwards_post_body`, `strips_hop_by_hop_both_ways` (CA 1–2).
- [x] T4. Errores. Tests: `upstream_down_is_502`, `origin_form_is_400`, `https_scheme_without_connect_is_400`, `self_request_is_508` (CA 3, 4, 6).
- [x] T5. CONNECT. Tests: `connect_tunnels_bytes`, `connect_unreachable_is_502`, `connect_to_self_is_508` (CA 5–6).
- [x] T6. CLI `proxyrr start [--listen]` + advertencia no-loopback. Tests: `proxyrr-cli/tests/start.rs` (CA 8–9).
- [ ] T7. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
