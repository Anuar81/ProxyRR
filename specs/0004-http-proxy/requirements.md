# 0004 — HTTP proxy

- Estado: en curso
- Fase: F1
- Depende de: 0001

## Objetivo

Primer proxy usable: apuntar un navegador (o el SO) a ProxyRR y ver pasar el tráfico en la terminal.
HTTP plano se reenvía y se registra; HTTPS pasa por un túnel `CONNECT` **sin descifrar** (el
descifrado llega en `https-mitm`), así la navegación sigue funcionando mientras tanto.

## Historias

**H1.** Como usuario, quiero correr `proxyrr start`, configurar mi navegador con ese proxy y ver cada
request HTTP (método, URL, status, tiempo, tamaño) en la terminal.

**H2.** Como usuario, quiero que los sitios HTTPS sigan cargando a través del proxy aunque todavía no se
descifren, y ver a qué hosts se conectan.

**H3.** Como motor, necesito publicar cada flujo como evento para que el CLI, la API y el store futuros
lo consuman sin tocar el núcleo.

## Criterios de aceptación

1. CUANDO llega un request en forma absoluta `http://…`, el proxy DEBE reenviarlo al origen en forma
   de origen (`/ruta?query`), con método, headers end-to-end y body intactos, y devolver status,
   headers end-to-end y body del origen sin modificar.
2. El proxy DEBE quitar los headers hop-by-hop en ambos sentidos: `Connection`, `Keep-Alive`,
   `Proxy-Connection`, `Proxy-Authenticate`, `Proxy-Authorization`, `TE`, `Trailer`,
   `Transfer-Encoding`, `Upgrade` y los que nombre el header `Connection`.
3. CUANDO el origen no responde o no se puede conectar, el proxy DEBE responder `502 Bad Gateway`
   con un texto que explique el error.
4. CUANDO llega un request en forma de origen (alguien abrió el proxy como si fuera un sitio) o con
   un esquema distinto de `http`, el proxy DEBE responder `400 Bad Request` con una explicación.
5. CUANDO llega `CONNECT host:puerto`, el proxy DEBE conectar al destino, responder `200` y copiar bytes
   en ambos sentidos hasta que alguno cierre. SI no puede conectar, DEBE responder `502`.
6. SI el destino de un request o `CONNECT` es el propio proxy (loopback + su puerto), el proxy DEBE
   responder `508 Loop Detected` en vez de reenviarse a sí mismo.
7. Por cada request HTTP terminado (o fallido) y cada túnel abierto (o fallido), el motor DEBE emitir un
   evento con id incremental, método, destino, status o error, duración hasta los headers de respuesta y
   tamaño declarado (`Content-Length`) si existe.
8. El proxy DEBE escuchar por defecto solo en `127.0.0.1:9090`. `--listen <ip:puerto>` lo cambia; con una
   IP que no sea loopback el CLI DEBE advertir que cualquiera en la red puede usar el proxy.
9. `proxyrr start` DEBE imprimir la dirección efectiva (útil con puerto `0`), una línea por evento, y
   terminar limpio con Ctrl+C.

## Fuera de alcance

Descifrado TLS (`https-mitm`), HTTP/2, WebSocket y `Upgrade` en HTTP plano (F5), autenticación del proxy,
proxy upstream/PAC (F5), captura de bodies y persistencia (`flow-store`), modificar tráfico (F3).
