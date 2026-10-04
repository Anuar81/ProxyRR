// Lógica pura de la UI (sin DOM ni Tauri): se prueba con `node --test crates/proxyrr-desktop/tests-ui/lib.test.mjs`.

/** Tamaño legible: 512 B, 1.5 KB, 3.0 MB. */
export function formatSize(bytes) {
  if (bytes === null || bytes === undefined) return "";
  const KB = 1024;
  const MB = KB * 1024;
  if (bytes >= MB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes >= KB) return `${(bytes / KB).toFixed(1)} KB`;
  return `${bytes} B`;
}

/** Milisegundos legibles: 42 ms, 1.25 s. */
export function formatMs(ms) {
  if (ms === null || ms === undefined) return "";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms} ms`;
}

/** Clase de color según el status. */
export function statusClass(flow) {
  if (flow.failed) return "st-fail";
  if (flow.in_progress) return "st-pending";
  const s = flow.status;
  if (s >= 500) return "st-5xx";
  if (s >= 400) return "st-4xx";
  if (s >= 300) return "st-3xx";
  return "st-2xx";
}

/**
 * Filtro de la lista. Palabras separadas por espacios, todas tienen que coincidir (sin distinguir
 * mayúsculas) con la URL, el método o el status. Formas especiales:
 * `-palabra` excluye; `status:4xx` / `status:404`; `method:post`.
 */
export function matchesFilter(flow, query) {
  // Un `-` solo es una exclusión a medio escribir: se ignora en vez de vaciar la lista.
  const terms = (query || "").trim().toLowerCase().split(/\s+/).filter((t) => t && t !== "-");
  const haystack = `${flow.method} ${flow.status} ${flow.url}`.toLowerCase();
  return terms.every((term) => {
    const negate = term.startsWith("-") && term.length > 1;
    const t = negate ? term.slice(1) : term;
    let hit;
    if (t.startsWith("status:")) {
      const want = t.slice(7);
      const s = String(flow.status);
      hit = /^\dxx$/.test(want) ? s[0] === want[0] : s === want;
    } else if (t.startsWith("method:")) {
      hit = flow.method.toLowerCase() === t.slice(7);
    } else {
      hit = haystack.includes(t);
    }
    return negate ? !hit : hit;
  });
}

/** JSON con sangría; si no parsea, el texto tal cual. */
export function prettyJson(text) {
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** Vista hex clásica: offset, 16 bytes y su ASCII. */
export function hexDump(bytes) {
  const lines = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = bytes.slice(off, off + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(chunk, (b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${off.toString(16).padStart(8, "0")}  ${hex.padEnd(47, " ")}  |${ascii}|`);
  }
  return lines.join("\n");
}

/** Base64 → bytes (en el navegador con `atob`, en Node con `Buffer`). */
export function base64ToBytes(b64) {
  if (typeof atob === "function") {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
    return out;
  }
  return Uint8Array.from(Buffer.from(b64, "base64"));
}

/** Host de una URL (o `host:puerto` de un túnel). */
export function hostOf(url) {
  // `example.com:443` (túnel) no es una URL: `new URL` lo tomaría como esquema `example.com:`.
  if (!url.includes("://")) return url;
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** Separa `listen` en host y puerto; el puerto es el que usan las guías. */
export function portOf(listen, fallback = 9090) {
  const match = /:(\d+)$/.exec((listen || "").trim());
  const port = match ? Number(match[1]) : NaN;
  return Number.isInteger(port) && port > 0 && port < 65536 ? port : fallback;
}

/**
 * `true` si `listen` solo acepta conexiones de esta máquina (127.x, `localhost`, `[::1]`):
 * un teléfono en la red no puede llegar a esa dirección.
 */
export function isLoopbackListen(listen) {
  const text = (listen || "").trim().toLowerCase();
  const host = text.startsWith("[") ? text.slice(1, text.indexOf("]")) : text.replace(/:\d+$/, "");
  return host === "" || host === "localhost" || host === "::1" || /^127\./.test(host);
}

/** `a, b ,, c` → `["a", "b", "c"]`. */
export function parseList(text) {
  return (text || "").split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
}

/**
 * Lista ordenada por id con actualización en el lugar. `upsert` devuelve dónde quedó el flujo, para que
 * la vista toque solo esa fila.
 */
export class FlowList {
  constructor() {
    this.byId = new Map();
    this.ids = [];
  }

  get size() {
    return this.ids.length;
  }

  clear() {
    this.byId.clear();
    this.ids = [];
  }

  /** @returns {{ index: number, isNew: boolean }} */
  upsert(flow) {
    const isNew = !this.byId.has(flow.id);
    this.byId.set(flow.id, flow);
    if (!isNew) return { index: this.indexOf(flow.id), isNew };
    // Casi siempre llega el id más alto: se agrega al final sin buscar.
    const last = this.ids[this.ids.length - 1];
    if (last === undefined || flow.id > last) {
      this.ids.push(flow.id);
      return { index: this.ids.length - 1, isNew };
    }
    const index = this.insertionPoint(flow.id);
    this.ids.splice(index, 0, flow.id);
    return { index, isNew };
  }

  get(id) {
    return this.byId.get(id);
  }

  indexOf(id) {
    const i = this.insertionPoint(id);
    return this.ids[i] === id ? i : -1;
  }

  insertionPoint(id) {
    let lo = 0;
    let hi = this.ids.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.ids[mid] < id) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  /** Flujos en orden, opcionalmente filtrados. */
  visible(query) {
    return this.ids.map((id) => this.byId.get(id)).filter((f) => matchesFilter(f, query));
  }

  lastId() {
    return this.ids.length ? this.ids[this.ids.length - 1] : undefined;
  }
}


// ---------- Reglas (spec 0011) ----------

/** Nombre visible de cada tipo de regla. */
export const ACTION_LABELS = {
  map_local: "Map Local",
  map_remote: "Map Remote",
  block: "Block",
  no_cache: "No Caching",
  breakpoint: "Breakpoint",
};

/** Headers `[[nombre, valor]]` → una línea `Nombre: valor` por header. */
export function headersToText(headers) {
  return (headers || []).map(([name, value]) => `${name}: ${value}`).join("\n");
}

/** Inversa de `headersToText`. Ignora líneas vacías y sin `:`; el valor puede tener `:`. */
export function parseHeaders(text) {
  const out = [];
  for (const line of (text || "").split(/\r?\n/)) {
    const i = line.indexOf(":");
    if (i <= 0) continue;
    const name = line.slice(0, i).trim();
    if (name) out.push([name, line.slice(i + 1).trim()]);
  }
  return out;
}

/** Acción por defecto de cada tipo, para el editor. */
export function defaultAction(type) {
  switch (type) {
    case "map_local":
      return { type, status: 200, headers: [["content-type", "application/json"]], body: "{}", file: null };
    case "map_remote":
      return { type, scheme: "http", host: "localhost", port: 3000, path: null, query: null, preserve_host: false };
    case "block":
      return { type, status: 403 };
    case "breakpoint":
      return { type, request: true, response: true };
    default:
      return { type: "no_cache" };
  }
}

/** Regla vacía (id 0: la asigna el backend al guardar). */
export function emptyRule(type = "map_local") {
  return { id: 0, name: "", enabled: true, method: null, url: "", regex: false, action: defaultAction(type) };
}

/** Qué hace una regla, en una línea. */
export function ruleSummary(rule) {
  const a = rule.action;
  switch (a.type) {
    case "map_local":
      return a.file ? `responde ${a.status} con ${a.file}` : `responde ${a.status} (${a.body.length} caracteres)`;
    case "map_remote": {
      const parts = [a.scheme && `${a.scheme}://`, a.host, a.port && `:${a.port}`, a.path, a.query && `?${a.query}`].filter(Boolean);
      return `→ ${parts.join("") || "(sin cambios)"}${a.preserve_host ? " · Host original" : ""}`;
    }
    case "block":
      return `responde ${a.status}`;
    case "breakpoint":
      return [a.request && "request", a.response && "response"].filter(Boolean).join(" + ") || "(nada)";
    default:
      return "sin caché";
  }
}

/** Campo de texto opcional: vacío → `null`. */
export function optional(text) {
  const t = (text || "").trim();
  return t ? t : null;
}

/** Número entero en rango, o `null` si está vacío o es inválido. */
export function optionalInt(text, min, max) {
  const t = (text || "").trim();
  if (!/^\d+$/.test(t)) return null;
  const n = Number(t);
  return n >= min && n <= max ? n : null;
}
