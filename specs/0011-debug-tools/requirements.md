# 0011 — Herramientas de depuración (F3)

- Estado: en curso
- Fase: F3
- Depende de: 0004, 0005, 0007, 0008, 0009
- Rama: `spec/0011-f3-debug-tools`
- Referencia de ideas: [PROXYMAN-COMPARISON.md](../PROXYMAN-COMPARISON.md) (solo su documentación pública)

## Objetivo

Modificar el tráfico en el camino, como en Proxyman: mockear respuestas, redirigir a otro servidor,
pausar y editar requests y responses, bloquear, desactivar la caché, y reenviar o crear requests. Todo
en una rama (pedido del usuario: un solo PR para F3).

## Historias

- **H1.** Como dev, quiero que un endpoint devuelva el JSON y el status que yo defina (Map Local), para
  probar casos borde sin esperar al backend.
- **H2.** Como dev, quiero mandar los requests de producción a mi localhost (Map Remote) sin tocar la app.
- **H3.** Como dev, quiero pausar un request o su response, editarlos y soltarlos (Breakpoint).
- **H4.** Como dev, quiero bloquear hosts (Block) y forzar respuestas frescas (No Caching).
- **H5.** Como dev, quiero reenviar un request, editarlo antes de reenviarlo, o crear uno de cero, y
  copiarlo como cURL.
- **H6.** Como dev, quiero crear cualquiera de esas reglas con clic derecho sobre un flujo, ya completada.

## Criterios de aceptación

1. Una **regla** tiene nombre, activa/inactiva, método opcional, patrón de URL (wildcard `*` o regex) y
   una acción. Si el patrón no tiene `?`, se compara contra la URL sin query. Las reglas se aplican en
   orden; la primera que responde (Map Local/Block) gana y la primera Map Remote gana; No Caching y
   Breakpoint se suman. Test: `proxyrr-rules` `matcher_*`, `plan_*`.
2. Las reglas se guardan en `<datos>/rules.json`, las comparten la app y el CLI, y un cambio aplica al
   próximo request sin reiniciar el proxy. Un regex inválido se rechaza con el nombre de la regla. Tests:
   `rules_persist_and_reload`, `invalid_regex_is_rejected`, `rules_apply_live`.
3. **Map Local**: el proxy responde con el status, headers y body de la regla (o el contenido de un
   archivo) sin ir al origen; `Content-Length` se recalcula. Test: `map_local_responds_without_origin`.
4. **Map Remote**: reemplaza esquema, host, puerto, path y/o query (los vacíos no se cambian), cambia el
   `Host` salvo "preservar Host", y sigue detectando bucles. Tests: `map_remote_redirects`, `map_remote_loop_is_detected`.
5. **Block**: responde el status de la regla (403 por defecto) sin ir al origen. **No Caching**: quita
   los validadores de caché del request y de la respuesta y agrega `Cache-Control: no-store`. Tests:
   `block_and_no_cache`.
6. **Breakpoint** (request y/o response): el flujo queda en pausa hasta que la app elige Ejecutar (con
   los cambios), Continuar (sin cambios) o Abortar (503). A los 5 minutos sin respuesta continúa sin
   cambios. Sin una UI escuchando (CLI) no pausa. El body a pausar tiene el límite de captura; si lo
   supera se responde 413 (request) o 502 (response). Tests: `breakpoint_*`.
7. El flujo muestra qué reglas lo modificaron. Lo capturado es lo que viajó: el request tal como salió
   hacia el origen y la respuesta tal como llegó al cliente; la URL es la que pidió el cliente. Test:
   `flow_lists_applied_rules`.
8. **Repeat / Compose**: reenviar un flujo, o un request editado o nuevo, pasa por el proxy (se captura y
   se le aplican las reglas) aunque sea HTTPS. Requiere el proxy prendido. Tests: `replay_*`.
9. **Copy as cURL** genera un comando bash que reproduce el request (método, URL, headers y body, con
   comillas seguras). Test: `tools::tests::curl_reproduces_the_request`.
10. En la app: diálogo **Reglas** (lista, activar, editar, borrar, crear), clic derecho sobre un flujo
    (Map Local, Map Remote, Breakpoint, Block, No Caching, Repetir, Editar y repetir, Copiar como cURL),
    editor de breakpoints en cola y diálogo **Compose**. Map Local creado desde un flujo arranca con su
    respuesta decodificada.
11. API: `GET`/`PUT /api/v1/rules` y `POST /api/v1/flows/{id}/replay`. CLI: `start` carga las reglas de
    `<datos>/rules.json` o de `--rules <archivo>` y avisa que los breakpoints no pausan sin la app.

## Fuera de alcance

Scripting JS, Map Local de directorio, Allow List, throttling, templates de mensajes, breakpoints por la
API y WebSocket/HTTP/2 (F5). Quedan en el roadmap y en `PROXYMAN-COMPARISON.md`.


## Agregado al cierre (prueba en un teléfono real)

Salió de probar con un teléfono físico y una app propia; va en la misma rama para un único PR.

- Guías de teléfono por Wi-Fi: si el proxy escucha en loopback, aviso y botón "Escuchar en la red".
- Menú contextual que no se cerraba (`[hidden]` perdía contra `display: flex`); `Esc` lo cierra.
- `CONNECT` descifrados ocultos por defecto (interruptor **CONNECT**); su detalle lista los requests a
  ese host. Un túnel descifrado que se cierra sin requests avisa "posible certificate pinning" y queda
  visible.
- Mensaje de "el cliente no confía en la CA" válido también para teléfonos.
- Copiar como cURL para PowerShell (TD-012, parcial).
- Breakpoints por la API: `GET /api/v1/breakpoints`, `POST /api/v1/breakpoints/{key}` y avisos
  `paused`/`resolved` por el WebSocket (TD-011).
- Licencia PolyForm Small Business confirmada con el texto completo (TD-005) y README de uso.
