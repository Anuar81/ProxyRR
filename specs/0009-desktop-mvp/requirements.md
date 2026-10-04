# 0009 — Desktop MVP

- Estado: terminada (PR #8)
- Fase: F2
- Depende de: 0006, 0007, 0008

## Objetivo

La primera versión de la app de escritorio (Tauri, una sola UI para Windows, macOS y Linux): prender el
proxy, ver los flujos en vivo, inspeccionar request y response, filtrar, y el menú **Certificado** de la 0006.
Es el punto en que ProxyRR se usa como Proxyman.

## Historias

**H1.** Como usuario, quiero abrir la app, tocar "Iniciar" y ver pasar el tráfico en una lista.

**H2.** Como usuario, quiero elegir un flujo y ver sus headers y su body legible (JSON formateado, gzip/br
decodificado, imágenes, hex para binarios).

**H3.** Como usuario, quiero filtrar la lista por texto, status o método.

**H4.** Como usuario, quiero instalar la CA en este equipo y ver cómo configurar mis dispositivos sin salir de la app.

## Criterios de aceptación

1. La app DEBE prender y apagar el proxy con la dirección, MITM y bypass elegidos, mostrar su estado, y
   conservar los flujos al apagarlo. Un error (puerto ocupado, dirección inválida) DEBE mostrarse sin cerrar la app.
2. La lista DEBE mostrar cada flujo apenas llega (id, método, status, URL, tamaño, tiempo), actualizarlo en su
   lugar cuando termina, colorear por status y seguir el final solo si el usuario no subió.
3. SI la ventana se atrasa y se pierden avisos, ENTONCES DEBE resincronizar la lista sola y avisar.
4. El inspector DEBE mostrar, por lado (request / response), los headers en orden y con repetidos, y el body:
   JSON formateado (con opción crudo), texto, imagen, hex para binarios, y avisos si se truncó, se cortó o
   no se pudo decodificar. `gzip`, `deflate` y `br` DEBEN decodificarse para mostrar, con un tope contra bombas
   de compresión.
5. El filtro DEBE aceptar palabras (todas deben coincidir), `-palabra`, `status:4xx`/`status:404` y `method:post`.
6. El menú **Certificado** DEBE tener "Este equipo" (estado por almacén, instalar/desinstalar, advertencia,
   aviso de Firefox) y guías para iPhone/iPad, simulador de iOS (con instalación automática en macOS),
   emulador y dispositivo Android, con QR y botones para copiar (0006, CA 14).
7. Nada que venga de un flujo capturado (headers, bodies, URLs) DEBE poder ejecutar código en la ventana:
   se inserta como texto, los SVG capturados no se renderizan como imagen y la CSP no permite scripts externos
   ni inline.
8. La app DEBE usar la misma CA y el mismo directorio de datos que el CLI.

## Fuera de alcance

Guardar/abrir sesiones, export HAR/cURL, búsqueda en bodies, columnas configurables, pestañas, reglas y
breakpoints (F3), aviso de donación (queda en el roadmap F2), empaquetado firmado e instaladores.
