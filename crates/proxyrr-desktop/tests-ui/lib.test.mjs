// Tests de la lógica de la UI: `node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs`.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  ACTION_LABELS,
  FlowList,
  authorityOf,
  base64ToBytes,
  emptyRule,
  formatMs,
  formatSize,
  headersToText,
  hexDump,
  hostOf,
  isLoopbackListen,
  isQuietTunnel,
  isShown,
  matchesFilter,
  optional,
  optionalInt,
  parseHeaders,
  parseList,
  portOf,
  prettyJson,
  requestsTo,
  ruleSummary,
  statusClass,
} from "../ui/lib.js";

const flow = (id, extra = {}) => ({
  id,
  method: "GET",
  url: `https://api.example.com/v1/items/${id}`,
  status: 200,
  failed: false,
  in_progress: false,
  ...extra,
});

test("formatos", () => {
  assert.equal(formatSize(512), "512 B");
  assert.equal(formatSize(1536), "1.5 KB");
  assert.equal(formatSize(3 * 1024 * 1024), "3.0 MB");
  assert.equal(formatSize(null), "");
  assert.equal(formatMs(42), "42 ms");
  assert.equal(formatMs(1250), "1.25 s");
});

test("clase de status", () => {
  assert.equal(statusClass(flow(1)), "st-2xx");
  assert.equal(statusClass(flow(1, { status: 404 })), "st-4xx");
  assert.equal(statusClass(flow(1, { status: 502, failed: true })), "st-fail");
  assert.equal(statusClass(flow(1, { in_progress: true })), "st-pending");
});

test("filtro", () => {
  const f = flow(7, { method: "POST", status: 404 });
  assert.ok(matchesFilter(f, ""));
  assert.ok(matchesFilter(f, "EXAMPLE items"));
  assert.ok(!matchesFilter(f, "example otra"), "todas las palabras");
  assert.ok(matchesFilter(f, "status:4xx"));
  assert.ok(matchesFilter(f, "status:404"));
  assert.ok(!matchesFilter(f, "status:2xx"));
  assert.ok(matchesFilter(f, "method:post"));
  assert.ok(!matchesFilter(f, "-example"));
  assert.ok(matchesFilter(f, "-google"));
  assert.ok(matchesFilter(f, "-"), "un guion solo (a medio escribir) no filtra");
});

test("json y hex", () => {
  assert.equal(prettyJson('{"a":[1]}'), '{\n  "a": [\n    1\n  ]\n}');
  assert.equal(prettyJson("no json"), "no json");
  const dump = hexDump(new Uint8Array([0x48, 0x6f, 0x6c, 0x61, 0x00, 0xff]));
  // 6 bytes = 17 caracteres de hex, rellenados a 47 (el ancho de 16 bytes).
  assert.equal(dump, "00000000  48 6f 6c 61 00 ff" + " ".repeat(47 - 17) + "  |Hola..|");
  assert.equal(hexDump(new Uint8Array(17)).split("\n").length, 2);
  assert.deepEqual(Array.from(base64ToBytes("SG9sYQ==")), [72, 111, 108, 97]);
});

test("utilidades", () => {
  assert.equal(hostOf("https://a.b:8443/x"), "a.b:8443");
  assert.equal(hostOf("example.com:443"), "example.com:443");
  assert.equal(portOf("0.0.0.0:9191"), 9191);
  assert.equal(portOf("basura"), 9090);
  assert.equal(portOf("[::1]:70000"), 9090);
  assert.deepEqual(parseList(" a.com, *.b.com  c.com,,"), ["a.com", "*.b.com", "c.com"]);
});

test("lista ordenada con actualización en el lugar", () => {
  const list = new FlowList();
  assert.deepEqual(list.upsert(flow(2)), { index: 0, isNew: true });
  assert.deepEqual(list.upsert(flow(5)), { index: 1, isNew: true });
  assert.deepEqual(list.upsert(flow(3)), { index: 1, isNew: true }, "llegó tarde: va en su lugar");
  assert.deepEqual(list.upsert(flow(3, { status: 500 })), { index: 1, isNew: false });
  assert.equal(list.get(3).status, 500);
  assert.deepEqual(list.ids, [2, 3, 5]);
  assert.equal(list.indexOf(4), -1);
  assert.equal(list.lastId(), 5);
  assert.deepEqual(list.visible("status:5xx").map((f) => f.id), [3]);
  list.clear();
  assert.equal(list.size, 0);
  assert.equal(list.lastId(), undefined);
});


test("headers en texto, ida y vuelta", () => {
  const headers = [["content-type", "application/json"], ["x-url", "https://a.b/c"]];
  const text = headersToText(headers);
  assert.equal(text, "content-type: application/json\nx-url: https://a.b/c");
  assert.deepEqual(parseHeaders(text), headers);
  assert.deepEqual(parseHeaders("\n sin-dos-puntos\n: sin nombre\nA:  1 \r\n"), [["A", "1"]]);
  assert.deepEqual(headersToText(null), "");
});

test("reglas: vacía, resumen y campos opcionales", () => {
  const rule = emptyRule("map_local");
  assert.equal(rule.id, 0);
  assert.equal(rule.action.status, 200);
  assert.equal(ruleSummary(rule), "responde 200 (2 caracteres)");
  assert.equal(ruleSummary(emptyRule("map_remote")), "→ http://localhost:3000");
  assert.equal(ruleSummary(emptyRule("block")), "responde 403");
  assert.equal(ruleSummary(emptyRule("breakpoint")), "request + response");
  assert.equal(ruleSummary(emptyRule("no_cache")), "sin caché");
  assert.equal(ACTION_LABELS.map_remote, "Map Remote");
  assert.equal(optional("  "), null);
  assert.equal(optional(" a "), "a");
  assert.equal(optionalInt("8080", 1, 65535), 8080);
  assert.equal(optionalInt("70000", 1, 65535), null);
  assert.equal(optionalInt("12a", 1, 65535), null);
});

test("isLoopbackListen: un teléfono no llega a loopback", () => {
  for (const addr of ["127.0.0.1:9090", "localhost:9090", "[::1]:9090", "", "127.5.0.1:1"]) {
    assert.equal(isLoopbackListen(addr), true, addr);
  }
  for (const addr of ["0.0.0.0:9090", "192.168.0.254:9090", "[::]:9090"]) {
    assert.equal(isLoopbackListen(addr), false, addr);
  }
});

test("CONNECT descifrados: se ocultan salvo que tengan aviso o se pidan", () => {
  const quiet = { id: 1, method: "CONNECT", url: "api.x.com:443", status: 200, tunnel: true, intercepted: true, failed: false };
  const warned = { ...quiet, id: 2, failed: true };
  const opaque = { ...quiet, id: 3, intercepted: false };
  const http = flow(4);
  assert.equal(isQuietTunnel(quiet), true);
  assert.equal(isShown(quiet, ""), false);
  assert.equal(isShown(quiet, "", true), true);
  assert.equal(isShown(warned, ""), true, "el aviso de pinning tiene que verse");
  assert.equal(isShown(opaque, ""), true);
  assert.equal(isShown(http, "items"), true);
  const list = new FlowList();
  for (const f of [quiet, warned, opaque, http]) list.upsert(f);
  assert.deepEqual(list.visible("").map((f) => f.id), [2, 3, 4]);
});

test("requestsTo: los requests al destino de un túnel", () => {
  assert.equal(authorityOf("https://API.x.com/v1?a=1"), "api.x.com:443");
  assert.equal(authorityOf("https://api.x.com:8443/"), "api.x.com:8443");
  assert.equal(authorityOf("http://api.x.com/"), "api.x.com:80");
  assert.equal(authorityOf("no es url"), "");
  const flows = [
    { id: 1, tunnel: true, url: "api.x.com:443" },
    { id: 2, url: "https://api.x.com/login" },
    { id: 3, url: "https://otro.com/" },
    { id: 4, url: "https://api.x.com/refresh" },
  ];
  assert.deepEqual(requestsTo(flows, "api.x.com:443").map((f) => f.id), [2, 4]);
});
