# 0005 — HTTPS MITM

- Estado: terminada (PR #4)
- Fase: F1
- Depende de: 0003, 0004

## Objetivo

Descifrar el tráfico HTTPS que pasa por `CONNECT`: el proxy termina el TLS del cliente con un
certificado hoja emitido al vuelo por la CA de ProxyRR (0003), reenvía cada request al origen por un
TLS nuevo y publica los flujos igual que en HTTP plano (0004). Así se ven URL, status y tamaño de
cualquier sitio o app HTTPS que confíe en la CA.

## Historias

**H1.** Como usuario, quiero correr `proxyrr start --mitm`, instalar la CA una vez y ver las URLs HTTPS
completas en la terminal.

**H2.** Como usuario, quiero excluir hosts del descifrado (apps con pinning, bancos) y que sigan
funcionando por túnel.

**H3.** Como usuario, si el navegador o la app no confían en la CA, quiero un mensaje que me diga eso,
no un fallo mudo.

## Criterios de aceptación

1. CUANDO el MITM está activo y llega `CONNECT host:puerto` con tráfico TLS, el proxy DEBE responder `200`,
   terminar el TLS con una hoja para el SNI (o para el host del `CONNECT` si no hay SNI, p. ej. IPs),
   firmada por la CA de ProxyRR y cacheada.
2. Cada request dentro del túnel descifrado DEBE reenviarse a `https://host:puerto` del `CONNECT` con método,
   headers end-to-end y body intactos (mismas reglas hop-by-hop que 0004), y la respuesta volver al cliente.
   Una conexión TLS del cliente DEBE poder llevar varios requests (keep-alive).
3. El proxy DEBE verificar el certificado del origen contra el store del SO (más raíces extra configurables).
   SI la verificación falla, ENTONCES DEBE responder `502` dentro del túnel con el motivo.
4. Por cada request descifrado DEBE emitirse un `HttpFlow` con la URL `https://…` completa.
5. SI el host coincide con la lista de bypass (`host` exacto o `*.dominio` para subdominios, sin distinguir
   mayúsculas), ENTONCES el `CONNECT` DEBE tunelizarse sin descifrar, como en 0004.
6. SI el primer byte del cliente no es un handshake TLS, o el cliente no manda nada en 5 s (protocolos donde
   habla primero el servidor), ENTONCES el proxy DEBE tunelizar sin descifrar, sin perder los bytes ya leídos.
7. SI el cliente rechaza el certificado de ProxyRR, ENTONCES el evento del túnel DEBE decir que el cliente no
   confía en la CA y cómo resolverlo.
8. El proxy DEBE ofrecer solo `http/1.1` por ALPN (HTTP/2 es F5): los clientes negocian HTTP/1.1.
9. Sin MITM activo, el comportamiento DEBE ser exactamente el de 0004.
10. El CLI DEBE ofrecer `proxyrr start --mitm [--bypass <host>]…`, usar la CA del directorio de datos
    (creándola si no existe) e indicar su huella y cómo exportarla para instalarla.

## Fuera de alcance

Instalar la CA en el SO o dispositivos (F2/F4), HTTP/2 y WebSocket sobre TLS (F5; hoy un `wss://` descifrado
falla: usar `--bypass`), certificados de cliente (mTLS), pinning (no se puede evitar desde el proxy).
