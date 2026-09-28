# 0007 — Flow store · Diseño

## Dónde vive cada cosa

- `proxyrr-core`: el motor **produce** los datos. Módulo `capture.rs` con `Recorder<B>` (un `Body` que
  reenvía cada frame y copia los datos) y `Tap` (junta el body de request y el de response de un flujo y
  emite `HttpBodies` cuando terminan los dos). Los tipos del evento (`HttpBodies`, `CapturedBody`) van en
  `event.rs`, junto a los que ya hay.
- `proxyrr-store`: el almacén **consume** eventos. Depende de `proxyrr-core`, no al revés.

## Captura

```
request  ──► Recorder(req body)  ──► cliente upstream ──► origen
respuesta ◄── Recorder(resp body) ◄────────────────────── origen / respuesta propia (400/502/508)
                    │                         │
                    └──── Tap (por flujo) ────┘ ── ambos terminados ──► FlowEvent::HttpBodies
```

- `Recorder::poll_frame` reenvía el frame tal cual y, si trae datos, copia hasta el límite.
- Fin del body: `poll_frame` devuelve `None`, **o** `is_end_stream()` pasa a `true` después de un frame
  (hyper no siempre vuelve a llamar con un body de tamaño exacto), **o** el `Recorder` se destruye
  (corte: `complete = is_end_stream()`).
- El `Tap` guarda el primer resultado de cada lado; cuando están los dos, emite una sola vez.
- Orden garantizado: `Http` se emite antes de devolver la respuesta a hyper, y el body de response no puede
  terminar antes; así `HttpBodies` siempre llega después de `Http` del mismo id.

## Almacén

```rust
FlowStore::new(StoreLimits { max_flows, max_body_bytes })
store.apply(&event)                 // junta por id
store.list() -> Vec<FlowSummary>    // orden de id
store.get(id) -> Option<StoredFlow> // Http(HttpRecord { head, bodies }) | Tunnel(TunnelFlow)
store.clear(); store.dropped_events()
proxyrr_store::record(store, receiver) -> JoinHandle  // task que aplica eventos del proxy
```

`BTreeMap<u64, StoredFlow>` + contador de bytes de bodies; al pasarse de un límite, `pop_first` hasta volver.
Los bodies son `Bytes`: clonar un flujo para la API no copia los datos.
