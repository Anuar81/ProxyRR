# 0007 — Flow store · Tareas

- [x] T1. Headers en `HttpFlow` + tipos `HttpBodies` / `CapturedBody` (CA 1).
- [x] T2. `Recorder` + `Tap`. Tests en `capture.rs`: `forwards_frames_unchanged_and_captures`, `truncates_at_limit_but_forwards_everything`, `exact_size_body_finishes_without_extra_poll`, `dropped_mid_body_is_incomplete`, `emits_only_when_both_sides_finish`, `headers_keep_order_and_duplicates` (CA 3–5).
- [x] T3. Captura cableada en el forward HTTP, MITM y respuestas propias. Tests: `http_proxy.rs::captures_headers_and_bodies`, `truncates_capture_but_forwards_everything`, `rejected_flow_is_captured`, `capture_does_not_buffer_the_stream`, `origin_cut_mid_body_is_incomplete`; `https_mitm.rs::decrypted_bodies_are_captured` (CA 1–6).
- [x] T4. `FlowStore`: junta, lista, get, clear, límites y eviction. Tests: `merges_head_and_bodies_by_id`, `lists_in_arrival_order_with_tunnels`, `orphan_bodies_are_ignored`, `evicts_oldest_by_count`, `evicts_oldest_by_body_bytes`, `clear_empties_everything` (CA 7, 8, 10).
- [x] T5. `record()` + conteo de eventos perdidos. Tests: `record_counts_dropped_events`, `tests/end_to_end.rs::proxy_flows_end_up_in_the_store` (CA 9).
- [x] T6. fmt, clippy, test, deny en verde local y en CI. PR. Marcar spec como `terminada`.
