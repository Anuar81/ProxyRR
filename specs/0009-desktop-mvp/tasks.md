# 0009 — Desktop MVP · Tareas

- [x] T1. Esqueleto Tauri + ícono + política de licencias (`deny.toml`, ADR 0001).
- [x] T2. `decode_body` (CA 4). Tests: `identity_and_missing_encoding`, `gzip_deflate_and_brotli`, `chained_encodings_are_undone_in_reverse`, `truncated_stream_is_partial_not_an_error`, `bombs_are_capped`, `unknown_or_garbage_falls_back_to_raw`.
- [x] T3. `Backend` (CA 1, 4, 6, 8). Tests: `body_views`, `long_text_is_capped_on_a_char_boundary`, `install_runs_the_plan_for_this_os`, `guides_and_simulator`, `capture_flow_end_to_end`.
- [x] T4. Comandos + evento de avisos (CA 1–3).
- [x] T5. UI: lista en vivo, inspector, filtro, diálogo Certificado (CA 2–7). Tests de `lib.js` (6) con `node --test`.
- [x] T6. Prueba manual en Windows: tráfico real (JSON, gzip, PNG, 404), inspector y diálogo Certificado (capturas en el PR).
- [x] T7. CI: dependencias de WebKitGTK en Linux + tests de UI.
- [ ] T8. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
