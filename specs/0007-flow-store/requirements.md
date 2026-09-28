# 0007 — Flow store

- Estado: terminada (PR #5)
- Fase: F1
- Depende de: 0004, 0005

## Objetivo

Capturar cada flujo **completo** (headers y bodies de request y response) sin frenar el tráfico, y
guardarlo en un almacén en memoria con límites, listo para que la API de control (`control-api`) y la
app de escritorio lo listen e inspeccionen. La persistencia en disco (guardar/abrir sesión) queda para
F3; esta spec deja el modelo preparado.

## Historias

**H1.** Como usuario, quiero ver el body de cualquier request o response que pasó por el proxy, también
los de HTTPS descifrado.

**H2.** Como usuario, quiero que el proxy no se coma la memoria aunque lo deje horas capturando.

**H3.** Como motor, necesito que capturar no retrase ni cambie lo que ve el cliente.

## Criterios de aceptación

1. El evento `Http` DEBE incluir los headers de request (como los mandó el cliente) y de response (como los
   mandó el origen), en orden y con repetidos.
2. CUANDO terminan los bodies de request y response de un flujo, el motor DEBE emitir un evento
   `HttpBodies` con ambos bodies, su tamaño real, si se truncaron, si terminaron completos y la duración total.
3. La captura DEBE ser en streaming: los bytes llegan al otro lado en cuanto pasan, sin esperar el body completo.
4. Cada body se captura hasta `max_body_capture` (10 MiB por defecto); SI se pasa, ENTONCES se sigue
   reenviando entero pero se guarda truncado y marcado como tal, con el tamaño real contado.
5. SI el cliente o el origen cortan a mitad de body, ENTONCES el evento `HttpBodies` DEBE emitirse igual,
   con lo capturado y `complete = false`.
6. Los flujos que el proxy rechaza (400/502/508) DEBEN capturarse igual, con la respuesta que generó el proxy.
7. `FlowStore` DEBE juntar los eventos por id y ofrecer: listar resúmenes en orden de llegada, obtener un flujo
   completo por id, y vaciar.
8. `FlowStore` DEBE respetar un máximo de flujos (10 000) y de bytes de bodies (512 MiB) por defecto; al
   pasarse, DEBE descartar los flujos más viejos primero.
9. SI el almacén se atrasa y el canal de eventos descarta mensajes, ENTONCES DEBE contarlos (para avisarlo en la UI).
10. El resumen de un flujo DEBE usar el tamaño real del body de response cuando se conoce (paga TD-002).

## Fuera de alcance

SQLite y guardar/abrir sesión (F3), export HAR (`har-export`), búsqueda full-text (F2), decodificar
`gzip`/`br` para mostrar (F2; se guarda el body tal cual viajó).
