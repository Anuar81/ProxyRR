# 0008 — Control API · Tareas

- [x] T1. `FlowStore::summary`, `list_after` y `note_dropped` público. Test: `list_after_and_summary`.
- [x] T2. `Engine` + `FlowIds` en el motor (CA 5). Tests: `http_proxy.rs::shared_flow_ids_never_repeat_across_instances`, `api.rs::capture_inspect_restart_and_clear`.
- [x] T3. `ApiServer`: bind loopback, token, Host, errores JSON (CA 1–3, 9). Tests: unitarios `tokens`, `constant_time_compare`, `hosts`, `query_params`; `api.rs::refuses_non_loopback_and_bad_tokens`, `requires_token_and_local_host`, `errors_are_json`.
- [x] T4. Rutas de status, proxy, flows y bodies (CA 4–7). Tests: `capture_inspect_restart_and_clear`, `mitm_starts_when_the_engine_has_a_ca`, `errors_are_json`.
- [x] T5. WebSocket `/events` (CA 8). Test: `websocket_streams_live_notices` (token por query y por header, `hello`/`proxy`/`flow`/`cleared`, cierre al apagar la API).
- [x] T6. `proxyrr start --api` + `PROXYRR_API_TOKEN` (CA 10). Tests: `start.rs::api_flag_serves_the_control_api_with_the_env_token`, `api_listen_requires_api`; los de la 0004/0005 siguen en verde.
- [ ] T7. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
