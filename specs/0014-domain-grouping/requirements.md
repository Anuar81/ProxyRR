# 0014 — Agrupar el tráfico por app y por dominio

- Estado: borrador
- Fase: F2 (app de escritorio)
- Depende de: 0007, 0008, 0009
- Rama: `spec/0014-domain-grouping` (un único PR al final)
- Referencia de ideas: la Source List de Proxyman ([guía de inicio](https://proxyman.io/posts/2019-06-14-getting-started-with-proxyman))
  y la barra lateral de TCP Viewer, de los mismos autores ([doc](https://tcpviewer.proxyman.com/docs/features/group-traffic)).
  Solo su documentación pública.

## Por qué

Hoy la app muestra una sola lista larga, y un dominio solo se aísla escribiéndolo en la búsqueda. Un
compañero que usa Proxyman todos los días pidió lo que más usa: un panel a la izquierda que agrupa el
tráfico **solo**, a medida que llega, por la **app** que lo generó y por el **dominio** al que fue.
Con eso el tráfico de su app del simulador queda aparte del resto de la Mac, `api.naranjax.com` junta
todos sus endpoints y `localhost:3000` queda en su propio grupo.

## Historias

- **H1.** Como dev, quiero ver los dominios que aparecieron, con cuántos flujos tiene cada uno, y
  tocar uno para ver solo su tráfico.
- **H2.** Como dev, quiero ver qué app de mi Mac (incluidas las del simulador de iOS) hizo cada
  request, y tocar la app para ver solo lo suyo, desplegado por dominio.
- **H3.** Como dev, quiero que un teléfono o emulador que pasa por el proxy aparezca como su propio
  grupo.
- **H4.** Como dev, quiero fijar arriba la app o el dominio que estoy depurando y exportar solo ese
  grupo como HAR.

## Criterios de aceptación

### Motor: quién hizo el request

1. Cada flujo guarda su **cliente**: la dirección IP y el puerto de origen de la conexión que lo trajo.
   Los requests que viajan dentro de un túnel descifrado heredan el cliente del túnel. El resumen del
   flujo (`FlowSummary`, `FlowSummaryDto`) lo expone como `client: { addr, app }`. Test:
   `client_addr_is_recorded`, `mitm_requests_inherit_tunnel_client`.
2. CUANDO la conexión viene de loopback en **macOS**, el proxy DEBE resolver qué proceso es dueño del
   puerto de origen y guardar su nombre en `client.app` (para una app `.app`, su nombre visible; para
   un binario suelto como `curl` o `node`, el nombre del ejecutable). Las apps del simulador de iOS
   son procesos de la Mac y salen con su propio nombre. Test: `resolves_own_process_on_macos`
   (el test se conecta al proxy y espera el nombre de su propio ejecutable).
3. La resolución se hace **una vez por conexión**, fuera del camino del request y con un tiempo
   máximo: SI no se resuelve a tiempo, el proceso ya no existe o no hay permiso, ENTONCES el flujo
   sigue normal con `app` vacío. Nunca demora ni rompe un request. Test: `unresolved_client_does_not_block`.
4. En Linux y Windows, `app` queda vacío por ahora y los flujos locales se agrupan como
   "Esta máquina". Se anota como deuda (TD-018) con el mecanismo de cada sistema.
5. API: `GET /api/v1/har` acepta `?ids=1,2,3` y exporta solo esos flujos. Sin el parámetro exporta
   todo, como hoy. Test: `har_exports_selected_ids`.

### App: el panel lateral

6. A la izquierda de la lista hay un panel **Fuentes** que se arma solo con el tráfico que llega, sin
   configurar nada, y se actualiza en vivo. Tiene, en este orden:
   - **Todos**, con el total;
   - **Fijados** (si hay alguno);
   - **Apps**: un renglón por app de esta Mac; desplegarla muestra los dominios a los que habló;
   - **Dispositivos**: un renglón por cliente que no es loopback, con su IP y, si el `User-Agent` lo
     deja ver, el sistema (`192.168.0.12 · Android`; el emulador de Android sale como su IP del lado
     de la Mac);
   - **Dominios**: un renglón por host, con el tráfico de todas las apps juntas; desplegarlo muestra
     sus paths sin query (`/v1/users`, `/v1/cards`), cada uno con su contador.

   Cada renglón muestra cuántos flujos tiene. Test (`lib.test.mjs`): `groupFlows_*`.
7. El **dominio** es el host en minúsculas, con el puerto solo si no es el de su esquema:
   `api.naranjax.com`, `localhost:3000` y `localhost:8080` son tres grupos; `https://x.com` y
   `x.com:443` (su túnel) son el mismo. Test: `domainKey_*`.
8. CUANDO el usuario elige un renglón, la lista DEBE mostrar solo los flujos de ese grupo (una app, un
   dispositivo, un dominio, un dominio dentro de una app, o un path), combinado con la búsqueda, los
   filtros (`status:2xx`, etc.) y el interruptor CONNECT que ya existen. **Todos** vuelve a la lista
   completa. Test: `inSource_*`.
9. El panel tiene su propio campo de búsqueda; al filtrar deja visibles los padres de lo que coincide,
   para saber de dónde viene. Test: `filterSources_*`.
10. Clic derecho sobre un renglón: **Fijar / Quitar de fijados**, **Copiar nombre** y **Exportar HAR
    de este grupo**. Lo fijado se guarda en `localStorage` y sobrevive a reiniciar la app.
11. El panel se puede ocultar con un botón, y la app recuerda si quedó oculto.
12. Agrupar 10 000 flujos tarda menos de 50 ms en Node, y un flujo nuevo actualiza los contadores sin
    recalcular todo el árbol. Test: `groupFlows_is_incremental`, `groupFlows_10k_under_budget`.
13. Limpiar la lista vacía también el panel (salvo los fijados, que quedan con contador 0).

## Fuera de alcance

- **Fijar = descifrar solo eso** (como Proxyman): cambia «Sin descifrar» de lista de excluidos a lista
  de incluidos. Va en su propia spec.
- Íconos de las apps, borrar los flujos de un grupo, arrastrar para fijar.
- Capturar apps que ignoran el proxy del sistema (TD-017, Network Extension).
- Resolver el proceso en Linux y Windows (TD-018).
