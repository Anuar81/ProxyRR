// UI de ProxyRR. Todo lo que viene de un flujo se inserta como texto (`textContent`), nunca como HTML:
// un header o un body capturado no puede inyectar nada en la ventana.
import {
  ACTION_LABELS,
  FlowList,
  base64ToBytes,
  emptyRule,
  defaultAction,
  formatMs,
  formatSize,
  headersToText,
  hexDump,
  hostOf,
  matchesFilter,
  optional,
  optionalInt,
  parseHeaders,
  parseList,
  portOf,
  prettyJson,
  ruleSummary,
  statusClass,
} from "./lib.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

/** Crea un elemento; los hijos string van como texto. */
function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

/** Texto con `código` entre backticks, como en las guías. */
function richText(text) {
  return text.split("`").map((part, i) => (i % 2 ? el("code", { text: part }) : part));
}

const state = {
  flows: new FlowList(),
  rows: new Map(),
  pending: new Set(),
  selected: null,
  side: "response",
  running: false,
  listen: "",
  busy: false,
};

function showError(message) {
  $("error").textContent = message ? String(message) : "";
}

async function call(command, args) {
  try {
    showError("");
    return await invoke(command, args);
  } catch (e) {
    showError(e);
    throw e;
  }
}

// ---------- Proxy ----------

function setProxy(proxy) {
  state.running = proxy.running;
  state.listen = proxy.listen || "";
  const toggle = $("toggle");
  toggle.textContent = proxy.running ? "Detener" : "Iniciar";
  toggle.setAttribute("aria-pressed", String(proxy.running));
  for (const id of ["listen", "mitm", "bypass"]) $(id).disabled = proxy.running;
  const label = $("proxy-state");
  label.classList.toggle("on", proxy.running);
  label.textContent = proxy.running
    ? `Escuchando en ${proxy.listen}${proxy.mitm ? " · HTTPS descifrado" : " · HTTPS en túnel"}`
    : "Apagado";
  $("empty").innerHTML = "";
  $("empty").append(
    proxy.running
      ? el("span", {}, "Configurá ", el("code", { text: proxy.listen }), " como proxy HTTP y HTTPS en tu navegador o sistema.")
      : el("span", {}, "Tocá ", el("b", { text: "Iniciar" }), " y configurá la dirección como proxy HTTP y HTTPS."),
  );
}

async function toggleProxy() {
  if (state.busy) return;
  state.busy = true;
  $("toggle").disabled = true;
  try {
    const proxy = state.running
      ? await call("stop_proxy")
      : await call("start_proxy", {
          request: { listen: $("listen").value, mitm: $("mitm").checked, bypass: parseList($("bypass").value) },
        });
    setProxy(proxy);
  } catch {
    // El error ya se mostró.
  } finally {
    state.busy = false;
    $("toggle").disabled = false;
  }
}

// ---------- Lista ----------

function fillRow(tr, f) {
  tr.className = [f.tunnel ? "tunnel" : "", f.id === state.selected ? "selected" : ""].join(" ").trim();
  const rules = f.rules && f.rules.length ? f.rules : null;
  tr.replaceChildren(
    el("td", { class: "num", text: f.id }),
    el("td", { text: f.method }),
    el("td", { class: statusClass(f), text: f.in_progress ? "…" : f.status }),
    el("td", { title: rules ? `${f.url}\nReglas: ${rules.join(", ")}` : f.url },
      rules ? el("span", { class: "badge", text: rules.length > 1 ? `${rules.length} reglas` : "regla" }) : "",
      f.url),
    el("td", { class: "num", text: formatSize(f.response_size) }),
    el("td", { class: "num", text: formatMs(f.elapsed_ms) }),
  );
}

function makeRow(f) {
  const tr = el("tr", {
    "data-id": f.id,
    onclick: () => select(f.id),
    oncontextmenu: (e) => {
      e.preventDefault();
      select(f.id);
      openMenu(e.clientX, e.clientY, state.flows.get(f.id) || f);
    },
  });
  fillRow(tr, f);
  state.rows.set(f.id, tr);
  return tr;
}

function renderAll() {
  const query = $("filter").value;
  state.rows.clear();
  const rows = state.flows.visible(query).map(makeRow);
  $("rows").replaceChildren(...rows);
  updateFooter();
}

/** Aplica los flujos pendientes una vez por frame (con mucho tráfico llegan decenas por segundo). */
function scheduleFlush() {
  if (state.flushing) return;
  state.flushing = true;
  requestAnimationFrame(() => {
    state.flushing = false;
    const query = $("filter").value;
    const tbody = $("rows");
    // Si la lista estaba al final, se sigue el tráfico nuevo; si el usuario subió, no se lo mueve.
    const list = document.querySelector(".list");
    const follow = list.scrollHeight - list.scrollTop - list.clientHeight < 24;
    for (const id of state.pending) {
      const f = state.flows.get(id);
      if (!f) continue;
      const existing = state.rows.get(id);
      const visible = matchesFilter(f, query);
      if (existing && visible) fillRow(existing, f);
      else if (existing) {
        existing.remove();
        state.rows.delete(id);
      } else if (visible) {
        const tr = makeRow(f);
        const index = state.flows.indexOf(id);
        // La fila visible siguiente con id mayor; casi siempre no hay ninguna y va al final.
        let next = null;
        for (let i = index + 1; i < state.flows.ids.length && !next; i += 1) next = state.rows.get(state.flows.ids[i]) || null;
        tbody.insertBefore(tr, next);
      }
      if (id === state.selected) refreshDetail();
    }
    state.pending.clear();
    updateFooter();
    if (follow) list.scrollTop = list.scrollHeight;
  });
}

function upsert(flow) {
  state.flows.upsert(flow);
  state.pending.add(flow.id);
  scheduleFlush();
  // Un request recién mandado desde Repetir/Compose se selecciona solo cuando aparece.
  if (flow.id === state.selectWhenSeen) {
    state.selectWhenSeen = null;
    requestAnimationFrame(() => select(flow.id));
  }
}

async function resync() {
  const flows = await call("list_flows", { after: null });
  state.flows.clear();
  for (const f of flows) state.flows.upsert(f);
  renderAll();
}

function updateFooter() {
  const total = state.flows.size;
  const shown = state.rows.size;
  $("count").textContent = shown === total ? `${total} flujos` : `${shown} de ${total} flujos`;
  $("empty").hidden = total > 0;
}

async function refreshStatus() {
  const status = await call("status");
  const dropped = $("dropped");
  dropped.hidden = status.dropped_events === 0;
  dropped.textContent = `${status.dropped_events} eventos perdidos por carga: la lista puede estar incompleta`;
}

// ---------- Detalle ----------

async function select(id) {
  state.selected = id;
  for (const [rid, tr] of state.rows) tr.classList.toggle("selected", rid === id);
  state.rows.get(id)?.scrollIntoView({ block: "nearest" });
  await refreshDetail();
}

async function refreshDetail() {
  const id = state.selected;
  if (id === null) return;
  let flow;
  try {
    flow = await call("get_flow", { id });
  } catch {
    state.selected = null;
    $("detail").hidden = true;
    $("no-selection").hidden = false;
    return;
  }
  if (id !== state.selected) return;
  $("no-selection").hidden = true;
  $("detail").hidden = false;
  if (flow.kind === "tunnel") {
    $("d-title").textContent = `CONNECT ${flow.authority}`;
    $("d-meta").textContent = `${flow.status} · ${formatMs(flow.elapsed_ms)} · ${flow.intercepted ? "descifrado" : "sin descifrar"}`;
    document.querySelector(".tabs").hidden = true;
    $("pane").replaceChildren(
      flow.error ? el("p", { class: "note", text: flow.error }) : "",
      el("p", {
        text: flow.intercepted
          ? "Los requests de este túnel aparecen como flujos https:// en la lista."
          : "Túnel opaco: el contenido no se descifró (bypass, MITM apagado o no es TLS).",
      }),
    );
    return;
  }
  document.querySelector(".tabs").hidden = false;
  $("d-title").textContent = `${flow.method} ${flow.url}`;
  const bits = [
    flow.in_progress ? "recibiendo…" : String(flow.status),
    `headers en ${formatMs(flow.elapsed_ms)}`,
    flow.duration_ms != null ? `total ${formatMs(flow.duration_ms)}` : null,
    flow.response.body ? `response ${formatSize(flow.response.body.size)}` : null,
    hostOf(flow.url),
  ].filter(Boolean);
  $("d-meta").textContent = bits.join(" · ");
  if (flow.error) $("d-meta").append(el("div", { class: "note", text: flow.error }));
  for (const tab of document.querySelectorAll(".tabs button")) {
    tab.setAttribute("aria-selected", String(tab.dataset.side === state.side));
  }
  const message = flow[state.side];
  const pane = [
    el("div", { class: "d-actions" }, flowActions(flow)),
    flow.rules && flow.rules.length ? el("p", { class: "note", text: `Modificado por: ${flow.rules.join(", ")}` }) : "",
    el("h3", { text: `Headers (${message.headers.length})` }),
    el("table", { class: "headers" }, message.headers.map(([name, value]) => el("tr", {}, el("td", { text: name }), el("td", { text: value })))),
  ];
  const bodyBox = el("div", {}, el("p", { class: "empty", text: flow.in_progress ? "Recibiendo el body…" : "Cargando…" }));
  pane.push(bodyBox);
  $("pane").replaceChildren(...pane);
  if (!flow.in_progress) await renderBody(id, state.side, bodyBox);
}

async function renderBody(id, side, box) {
  let body;
  try {
    body = await invoke("get_body", { id, side });
  } catch (e) {
    box.replaceChildren(el("p", { class: "note", text: String(e) }));
    return;
  }
  if (id !== state.selected || side !== state.side) return;
  const title = el("h3", {}, `Body · ${formatSize(body.size)}`, body.content_type ? ` · ${body.content_type}` : "", body.content_encoding ? ` · ${body.content_encoding}` : "");
  const parts = [title];
  if (body.note) parts.push(el("p", { class: "note", text: body.note }));
  const pre = (text) => el("pre", { class: "body", text });
  const copyButton = (text) => el("button", { class: "small", type: "button", onclick: () => copy(text) }, "Copiar");
  switch (body.kind) {
    case "empty":
      parts.push(el("p", { class: "empty", text: "(vacío)" }));
      break;
    case "json": {
      const pretty = prettyJson(body.text);
      const view = pre(pretty);
      let raw = false;
      const toggle = el("button", { class: "small", type: "button" }, "Ver crudo");
      toggle.addEventListener("click", () => {
        raw = !raw;
        view.textContent = raw ? body.text : pretty;
        toggle.textContent = raw ? "Ver formateado" : "Ver crudo";
      });
      title.append(toggle, copyButton(body.text));
      parts.push(view);
      break;
    }
    case "text":
      title.append(copyButton(body.text));
      parts.push(pre(body.text));
      break;
    case "image": {
      // El MIME sale del header pero solo se usa en un data: URL de <img> (image/svg+xml se muestra como texto).
      const mime = (body.content_type || "image/png").split(";")[0].trim();
      parts.push(el("img", { class: "body-img", alt: "imagen del body", src: `data:${mime};base64,${body.base64}` }));
      break;
    }
    default:
      parts.push(pre(hexDump(base64ToBytes(body.base64))));
  }
  box.replaceChildren(...parts);
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch (e) {
    showError(`no se pudo copiar: ${e}`);
  }
}

function moveSelection(delta) {
  const ids = [...state.rows.keys()].sort((a, b) => a - b);
  if (!ids.length) return;
  const i = ids.indexOf(state.selected);
  const next = ids[Math.min(ids.length - 1, Math.max(0, i === -1 ? 0 : i + delta))];
  select(next);
}

// ---------- Certificado ----------

async function showCert(target) {
  for (const b of document.querySelectorAll(".dlg-nav button")) b.setAttribute("aria-current", String(b.dataset.target === target));
  const body = $("cert-body");
  body.replaceChildren(el("p", { class: "empty", text: "Cargando…" }));
  try {
    body.replaceChildren(
      ...(target === "this" ? await thisComputer() : target === "android" ? await androidPanel() : await deviceGuide(target)),
    );
  } catch (e) {
    body.replaceChildren(el("p", { class: "note", text: String(e) }));
  }
}

async function thisComputer() {
  const ca = await call("ca_overview");
  const action = (label, command) =>
    el("button", {
      type: "button",
      class: "primary",
      onclick: async (ev) => {
        ev.target.disabled = true;
        try {
          await call(command);
        } catch {
          // mostrado
        }
        showCert("this");
      },
    }, label);
  return [
    el("h2", { text: "Este equipo" }),
    el("p", {}, el("b", { text: ca.name }), el("br"), el("small", { text: `SHA-256 ${ca.sha256}` })),
    el("ul", { class: "stores" }, ca.stores.map((s) =>
      el("li", { class: s.installed === true ? "yes" : s.installed === false ? "no" : "unk" },
        s.store, s.installed === null ? ` (no se pudo verificar: ${s.detail || "sin detalle"})` : ""))),
    el("p", { class: "callout" }, richText(ca.warning)),
    el("div", { class: "actions" },
      ca.installed ? action("Desinstalar", "ca_uninstall") : action("Instalar como raíz de confianza", "ca_install")),
    el("p", { class: "d-meta", text: "El sistema puede pedirte confirmación o tu contraseña en su propio diálogo." }),
    ca.firefox_note ? el("p", { class: "note", text: ca.firefox_note }) : "",
  ];
}

async function deviceGuide(target) {
  const port = portOf(state.listen || $("listen").value);
  const g = await call("guide", { target, port });
  const parts = [el("h2", { text: g.title })];
  if (g.qr_svg) {
    const qr = el("div", { class: "qr", role: "img", "aria-label": `QR para abrir ${g.qr_url}` });
    // SVG generado por ProxyRR a partir de una URL propia (no de datos capturados).
    qr.innerHTML = g.qr_svg;
    parts.push(el("div", { class: "actions" }, qr, el("p", {}, "Escaneá con la cámara para abrir ", el("code", { text: g.qr_url }))));
  }
  parts.push(el("ol", {}, g.steps.map((s) => el("li", {}, richText(s)))));
  if (g.can_install) {
    parts.push(el("div", { class: "actions" }, el("button", {
      type: "button",
      class: "primary",
      onclick: async (ev) => {
        ev.target.disabled = true;
        try {
          await call("install_ios_simulator");
          ev.target.textContent = "Instalada ✔";
        } catch {
          ev.target.disabled = false;
        }
      },
    }, "Instalar en el simulador abierto")));
  }
  for (const [name, code] of g.snippets) {
    parts.push(el("h3", {}, name, el("button", { class: "small", type: "button", onclick: () => copy(code) }, "Copiar")), el("pre", { class: "body", text: code }));
  }
  for (const note of g.notes) parts.push(el("p", { class: "note" }, richText(note)));
  return parts;
}

// ---------- Android (adb) ----------

const DEVICE_STATE = {
  ready: "",
  unauthorized: "Sin autorizar: aceptá «¿Permitir depuración USB?» en el teléfono.",
  offline: "Desconectado o arrancando.",
};

async function androidPanel() {
  const list = await call("android_devices");
  const port = portOf(state.listen || $("listen").value);
  const parts = [
    el("h2", { text: "Android con un clic" }),
    el("p", {}, "ProxyRR configura el proxy del dispositivo y le instala la CA por ", el("code", { text: "adb" }),
      ". Al cerrar la app el proxy se quita solo (si no, el dispositivo se queda sin internet)."),
  ];
  if (!state.running) {
    parts.push(el("p", { class: "callout" }, "Iniciá el proxy primero (con «Descifrar HTTPS» para ver el tráfico de las apps)."));
  }
  if (list.error) parts.push(el("p", { class: "note", text: list.error }));

  const input = el("input", { value: list.adb || "", spellcheck: "false", size: "48", "aria-label": "Ruta de adb", placeholder: "Ruta de adb (vacío: buscarlo solo)" });
  parts.push(el("div", { class: "actions" },
    el("label", { class: "field grow-s" }, "adb ", input),
    el("button", { type: "button", onclick: async () => { try { await call("android_set_adb", { path: input.value }); } catch { return; } showCert("android"); } }, "Usar esta ruta"),
    el("button", { type: "button", onclick: () => showCert("android") }, "Actualizar"),
  ));

  if (!list.error && list.devices.length === 0) {
    parts.push(el("p", { class: "empty", text: "No hay emuladores ni dispositivos conectados. Abrí un emulador desde Android Studio o conectá un teléfono con la depuración USB activada, y tocá Actualizar." }));
  }
  for (const d of list.devices) {
    const result = el("div");
    const kind = d.emulator ? "Emulador" : "Dispositivo";
    const mode = d.rootable
      ? "CA de sistema: la ven todas las apps."
      : "CA de usuario: Chrome y tus apps con network_security_config.";
    const button = (label, onclick) => el("button", { type: "button", class: "primary", disabled: d.state !== "ready" || state.busy, onclick }, label);
    const configure = async (ev) => {
      ev.target.disabled = true;
      result.replaceChildren(el("p", { class: "empty", text: "Configurando… (adb root puede tardar unos segundos)" }));
      try {
        const done = await call("android_configure", { serial: d.serial, port });
        result.replaceChildren(
          el("p", { class: "ok" }, `✔ Proxy ${done.proxy}. `, done.system_ca ? "CA en el store del sistema (hasta reiniciar el emulador)." : "CA copiada al dispositivo."),
          ...done.notes.map((n) => el("p", { class: "note" }, richText(n))),
          el("p", { class: "d-meta", text: "Si una app ya estaba abierta, cerrala y volvé a abrirla." }),
        );
      } catch (e) {
        result.replaceChildren(el("p", { class: "note", text: String(e) }));
      }
      ev.target.disabled = false;
    };
    const revert = async (ev) => {
      ev.target.disabled = true;
      try {
        await call("android_revert", { serial: d.serial });
      } catch {
        ev.target.disabled = false;
        return;
      }
      showCert("android");
    };
    parts.push(el("section", { class: "device" },
      el("h3", {}, `${kind}: ${d.label}`, el("small", { text: ` ${d.serial}` })),
      el("p", { class: "d-meta", text: DEVICE_STATE[d.state] ?? d.state }),
      d.state === "ready" ? el("p", { text: mode }) : "",
      el("div", { class: "actions" },
        button(d.configured ? "Configurar de nuevo" : "Configurar", configure),
        d.configured || d.state === "ready" ? el("button", { type: "button", onclick: revert }, "Quitar proxy") : ""),
      result,
    ));
  }
  return parts;
}

async function exportHar() {
  try {
    const path = await call("export_har");
    $("info").textContent = `HAR guardado en ${path}`;
  } catch {
    // mostrado
  }
}

function showLog(level, message) {
  const log = $("log");
  log.hidden = false;
  log.className = level === "error" ? "error" : "warn";
  log.textContent = `Motor: ${message}`;
  log.title = message;
}

// ---------- Herramientas: menú, reglas, breakpoints, compose (spec 0011) ----------

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

function showInfo(text) {
  $("info").textContent = text;
}

/** Acciones sobre un flujo HTTP (menú contextual y botones del detalle). */
function flowCommands(flow) {
  if (flow.tunnel || flow.kind === "tunnel") return [];
  const draft = (kind) => () => newRuleFrom(flow.id, kind);
  return [
    ["Repetir", () => replay(flow.id)],
    ["Editar y repetir…", () => openCompose(flow.id)],
    ["Copiar como cURL", () => copyCurl(flow.id)],
    null,
    ["Map Local…", draft("map_local")],
    ["Map Remote…", draft("map_remote")],
    ["Breakpoint…", draft("breakpoint")],
    ["Block…", draft("block")],
    ["No Caching…", draft("no_cache")],
  ];
}

function flowActions(flow) {
  return flowCommands(flow)
    .filter((c) => c && ["Repetir", "Editar y repetir…", "Copiar como cURL", "Map Local…", "Breakpoint…"].includes(c[0]))
    .map(([label, run]) => el("button", { class: "small", type: "button", onclick: run }, label));
}

function openMenu(x, y, flow) {
  const commands = flowCommands(flow);
  if (!commands.length) return;
  const menu = $("menu");
  menu.replaceChildren(
    ...commands.map((c) =>
      c === null
        ? el("hr")
        : el("button", { type: "button", role: "menuitem", onclick: () => { closeMenu(); c[1](); } }, c[0])),
  );
  menu.hidden = false;
  // Que no se salga de la ventana.
  const { innerWidth: w, innerHeight: h } = window;
  const rect = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(x, w - rect.width - 4)}px`;
  menu.style.top = `${Math.min(y, h - rect.height - 4)}px`;
  menu.querySelector("button")?.focus();
}

function closeMenu() {
  $("menu").hidden = true;
}

async function replay(id) {
  try {
    state.selectWhenSeen = await call("replay_flow", { id });
    showInfo(`Repetido como #${state.selectWhenSeen}`);
  } catch {
    // mostrado
  }
}

async function copyCurl(id) {
  try {
    await copy(await call("copy_curl", { id }));
    showInfo("Comando cURL copiado (bash/zsh)");
  } catch {
    // mostrado
  }
}

// --- Reglas ---

async function openRules() {
  $("rules-dialog").showModal();
  await showRuleList();
}

async function showRuleList() {
  const rules = await call("rules_list");
  const body = $("rules-body");
  const save = async (next) => {
    try {
      await call("rules_save", { rules: next });
    } catch {
      return;
    }
    showRuleList();
  };
  const move = (i, delta) => {
    const next = rules.slice();
    const [r] = next.splice(i, 1);
    next.splice(i + delta, 0, r);
    save(next);
  };
  const rows = rules.map((r, i) =>
    el("tr", { class: r.enabled ? "" : "off" },
      el("td", {}, el("input", {
        type: "checkbox", checked: r.enabled, "aria-label": `Activar ${r.name}`,
        onchange: (e) => save(rules.map((x) => (x.id === r.id ? { ...x, enabled: e.target.checked } : x))),
      })),
      el("td", {}, el("span", { class: "kind", text: ACTION_LABELS[r.action.type] || r.action.type })),
      el("td", {}, el("b", { text: r.name }), el("div", { class: "hint", text: ruleSummary(r) })),
      el("td", { class: "pat", text: `${r.method || "*"} ${r.url}${r.regex ? "  (regex)" : ""}` }),
      el("td", { class: "ops" },
        el("button", { class: "small", type: "button", disabled: i === 0, "aria-label": "Subir", onclick: () => move(i, -1) }, "↑"),
        el("button", { class: "small", type: "button", disabled: i === rules.length - 1, "aria-label": "Bajar", onclick: () => move(i, 1) }, "↓"),
        el("button", { class: "small", type: "button", onclick: () => showRuleEditor(r, rules) }, "Editar"),
        el("button", { class: "small", type: "button", onclick: () => save(rules.filter((x) => x.id !== r.id)) }, "Borrar")),
    ));
  body.replaceChildren(
    el("h2", { id: "rules-title", text: "Reglas" }),
    el("p", { class: "hint" },
      "Se aplican en orden a cada request: la primera Map Local o Block que coincide responde sin ir al origen, " +
      "la primera Map Remote redirige, y No Caching y Breakpoint se suman. Se guardan solas y valen también para ",
      el("code", { text: "proxyrr start" }), "."),
    el("div", { class: "actions" },
      ...Object.entries(ACTION_LABELS).map(([type, label]) =>
        el("button", { type: "button", onclick: () => showRuleEditor(emptyRule(type), rules) }, `+ ${label}`))),
    rules.length
      ? el("table", { class: "rules-table" }, rows)
      : el("p", { class: "empty", text: "Sin reglas. Creá una con los botones de arriba o con clic derecho sobre un flujo." }),
  );
}

async function newRuleFrom(id, kind) {
  try {
    const draft = await call("rule_draft", { id, kind });
    const rules = await call("rules_list");
    $("rules-dialog").showModal();
    showRuleEditor(draft, rules);
  } catch {
    // mostrado
  }
}

/** Editor de una regla. `rules` es la lista actual: la regla se agrega o reemplaza y se guarda todo. */
function showRuleEditor(rule, rules) {
  const isNew = !rules.some((r) => r.id === rule.id && rule.id !== 0);
  let action = structuredClone(rule.action);
  const input = (value, attrs = {}) => el("input", { value: value ?? "", spellcheck: "false", ...attrs });
  const name = input(rule.name, { "aria-label": "Nombre" });
  const enabled = el("input", { type: "checkbox", checked: rule.enabled, "aria-label": "Activa" });
  const method = el("select", { "aria-label": "Método" },
    el("option", { value: "", text: "Cualquiera" }),
    ...METHODS.map((m) => el("option", { value: m, text: m, selected: (rule.method || "").toUpperCase() === m })));
  const url = input(rule.url, { "aria-label": "Patrón de URL", placeholder: "https://api.ejemplo.com/v1/*" });
  const regex = el("input", { type: "checkbox", checked: rule.regex, "aria-label": "Es regex" });
  const type = el("select", { "aria-label": "Tipo" },
    ...Object.entries(ACTION_LABELS).map(([value, label]) => el("option", { value, text: label, selected: value === action.type })));
  const fields = el("div", { class: "form wide" });
  const read = {};

  const renderFields = () => {
    const a = action;
    const parts = [];
    for (const k of Object.keys(read)) delete read[k];
    const row = (label, node) => parts.push(el("label", { text: label }), node);
    if (a.type === "map_local") {
      const status = input(a.status, { size: "5", "aria-label": "Status" });
      const headers = el("textarea", { rows: "4", "aria-label": "Headers", spellcheck: "false" });
      headers.value = headersToText(a.headers);
      const bodyText = el("textarea", { rows: "10", "aria-label": "Body", spellcheck: "false" });
      bodyText.value = a.body || "";
      const file = input(a.file, { "aria-label": "Archivo", placeholder: "C:\\mocks\\usuarios.json (opcional)" });
      row("Status", status);
      row("Headers", headers);
      row("Body", bodyText);
      row("…o archivo", el("div", {}, file, el("p", { class: "hint", text: "Si hay archivo, se lee en cada request y gana sobre el body." })));
      read.action = () => ({
        type: "map_local",
        status: optionalInt(status.value, 100, 999) ?? 200,
        headers: parseHeaders(headers.value),
        body: bodyText.value,
        file: optional(file.value),
      });
    } else if (a.type === "map_remote") {
      const scheme = el("select", { "aria-label": "Esquema" },
        ...[["", "(igual)"], ["http", "http"], ["https", "https"]].map(([v, t]) => el("option", { value: v, text: t, selected: (a.scheme || "") === v })));
      const host = input(a.host, { placeholder: "(igual)", "aria-label": "Host" });
      const port = input(a.port, { size: "6", placeholder: "(igual)", "aria-label": "Puerto" });
      const path = input(a.path, { placeholder: "(igual)", "aria-label": "Path" });
      const query = input(a.query, { placeholder: "(igual)", "aria-label": "Query" });
      const keepHost = el("input", { type: "checkbox", checked: a.preserve_host, "aria-label": "Mantener Host" });
      row("Esquema", scheme);
      row("Host", host);
      row("Puerto", port);
      row("Path", path);
      row("Query", query);
      row("", el("label", { class: "check" }, keepHost, " Mantener el header Host original"));
      parts.push(el("p", { class: "hint wide", text: "Los campos vacíos no cambian. Ej.: esquema http + host localhost + puerto 3000 manda producción a tu servidor local con el mismo path." }));
      read.action = () => ({
        type: "map_remote",
        scheme: optional(scheme.value),
        host: optional(host.value),
        port: optionalInt(port.value, 1, 65535),
        path: optional(path.value),
        query: optional(query.value),
        preserve_host: keepHost.checked,
      });
    } else if (a.type === "block") {
      const status = input(a.status, { size: "5", "aria-label": "Status" });
      row("Responder", status);
      read.action = () => ({ type: "block", status: optionalInt(status.value, 100, 999) ?? 403 });
    } else if (a.type === "breakpoint") {
      const req = el("input", { type: "checkbox", checked: a.request, "aria-label": "Pausar request" });
      const res = el("input", { type: "checkbox", checked: a.response, "aria-label": "Pausar response" });
      row("Pausar", el("div", { class: "row" }, el("label", { class: "check" }, req, " request"), el("label", { class: "check" }, res, " response")));
      parts.push(el("p", { class: "hint wide", text: "El flujo espera hasta que elijas Ejecutar, Continuar o Abortar (5 minutos como mucho). Solo pausa con la app abierta." }));
      read.action = () => ({ type: "breakpoint", request: req.checked, response: res.checked });
    } else {
      parts.push(el("p", { class: "hint wide", text: "Quita If-None-Match/If-Modified-Since del request y ETag/Last-Modified/Expires de la respuesta, y agrega Cache-Control: no-store." }));
      read.action = () => ({ type: "no_cache" });
    }
    fields.replaceChildren(...parts);
  };
  type.addEventListener("change", () => {
    action = defaultAction(type.value);
    renderFields();
  });
  renderFields();

  const submit = async () => {
    const edited = {
      ...rule,
      name: name.value.trim(),
      enabled: enabled.checked,
      method: method.value || null,
      url: url.value.trim(),
      regex: regex.checked,
      action: read.action(),
    };
    const next = isNew ? [...rules, edited] : rules.map((r) => (r.id === rule.id ? edited : r));
    try {
      await call("rules_save", { rules: next });
    } catch {
      return;
    }
    showInfo(`Regla "${edited.name || edited.url}" guardada`);
    showRuleList();
  };

  $("rules-body").replaceChildren(
    el("h2", { id: "rules-title", text: isNew ? "Nueva regla" : "Editar regla" }),
    el("div", { class: "form" },
      el("label", { text: "Nombre" }), name,
      el("label", { text: "Activa" }), enabled,
      el("label", { text: "Método" }), method,
      el("label", { text: "URL" }), el("div", {}, url,
        el("p", { class: "hint", text: "* es cualquier cosa. Sin ? en el patrón, la query no cuenta. Sin esquema, vale http y https." })),
      el("label", { text: "Regex" }), el("label", { class: "check" }, regex, " el patrón es una expresión regular"),
      el("label", { text: "Tipo" }), type,
      fields),
    el("div", { class: "actions" },
      el("button", { type: "button", class: "primary", onclick: submit }, "Guardar"),
      el("button", { type: "button", onclick: showRuleList }, "Cancelar")),
  );
  name.focus();
}

// --- Breakpoints ---

const paused = { list: [], current: null };

function updatePausedBadge() {
  const badge = $("paused");
  badge.hidden = paused.list.length === 0;
  badge.textContent = `⏸ ${paused.list.length} en pausa`;
}

function onBreakpoint(message) {
  if (message.type === "paused") {
    paused.list.push(message.flow);
    updatePausedBadge();
    if (!$("bp-dialog").open) $("bp-dialog").showModal();
    if (paused.current === null) paused.current = message.flow.key;
    renderBreakpoints();
  } else if (message.type === "resolved") {
    paused.list = paused.list.filter((p) => p.key !== message.key);
    if (paused.current === message.key) paused.current = paused.list[0]?.key ?? null;
    updatePausedBadge();
    renderBreakpoints();
  } else if (message.type === "lagged") {
    loadPaused().catch(() => {});
  }
}

async function loadPaused() {
  paused.list = await call("breakpoints_pending");
  if (!paused.list.some((p) => p.key === paused.current)) paused.current = paused.list[0]?.key ?? null;
  updatePausedBadge();
  renderBreakpoints();
}

function renderBreakpoints() {
  if (!$("bp-dialog").open) return;
  $("bp-queue").replaceChildren(...paused.list.map((p) =>
    el("button", { type: "button", "aria-current": String(p.key === paused.current), onclick: () => { paused.current = p.key; renderBreakpoints(); } },
      `${p.stage === "request" ? "→" : "←"} #${p.id} ${p.method} ${p.url}`)));
  const p = paused.list.find((x) => x.key === paused.current);
  const body = $("bp-body");
  if (!p) {
    body.replaceChildren(el("p", { class: "empty", text: "No hay flujos en pausa." }));
    return;
  }
  const isRequest = p.stage === "request";
  const method = el("select", { "aria-label": "Método" }, ...[...new Set([p.method, ...METHODS])].map((m) => el("option", { value: m, text: m, selected: m === p.method })));
  const url = el("input", { value: p.url, spellcheck: "false", "aria-label": "URL" });
  const status = el("input", { value: p.status ?? "", size: "5", "aria-label": "Status" });
  const headers = el("textarea", { rows: "8", spellcheck: "false", "aria-label": "Headers" });
  headers.value = headersToText(p.headers);
  const text = el("textarea", { rows: "14", spellcheck: "false", "aria-label": "Body" });
  text.value = p.body ?? "";
  const resolve = async (action) => {
    const edited = action === "execute"
      ? {
          method: isRequest ? method.value : null,
          url: isRequest ? url.value.trim() : null,
          status: isRequest ? null : optionalInt(status.value, 100, 999),
          headers: parseHeaders(headers.value),
          body: p.body === null ? null : text.value,
        }
      : null;
    try {
      await call("breakpoint_resolve", { key: p.key, action, edited });
    } catch {
      loadPaused().catch(() => {});
    }
  };
  body.replaceChildren(
    el("h2", { text: isRequest ? `Request #${p.id} en pausa` : `Response #${p.id} en pausa` }),
    isRequest ? "" : el("p", { class: "d-meta", text: `${p.method} ${p.url}` }),
    p.note ? el("p", { class: "note", text: p.note }) : "",
    el("div", { class: "form" },
      ...(isRequest
        ? [el("label", { text: "Método" }), method, el("label", { text: "URL" }), url]
        : [el("label", { text: "Status" }), status]),
      el("label", { text: "Headers" }), headers,
      el("label", { text: `Body (${formatSize(p.size)})` }),
      p.body === null ? el("p", { class: "hint", text: "Binario: no se edita acá." }) : text),
    el("div", { class: "actions" },
      el("button", { type: "button", class: "primary", onclick: () => resolve("execute") }, "Ejecutar con cambios"),
      el("button", { type: "button", onclick: () => resolve("continue") }, "Continuar sin cambios"),
      el("button", { type: "button", onclick: () => resolve("abort") }, "Abortar (503)")),
    el("p", { class: "hint", text: "Si no decidís en 5 minutos, sigue sin cambios. Cerrar esta ventana no lo suelta." }),
  );
}

// --- Compose ---

async function openCompose(fromId) {
  let request = { method: "GET", url: "https://", headers: [["accept", "*/*"]], body: "", note: null };
  if (fromId !== undefined) {
    try {
      request = await call("compose_from", { id: fromId });
    } catch {
      return;
    }
  }
  const method = el("select", { "aria-label": "Método" }, ...[...new Set([request.method, ...METHODS])].map((m) => el("option", { value: m, text: m, selected: m === request.method })));
  const url = el("input", { value: request.url, spellcheck: "false", "aria-label": "URL" });
  const headers = el("textarea", { rows: "8", spellcheck: "false", "aria-label": "Headers" });
  headers.value = headersToText(request.headers);
  const text = el("textarea", { rows: "12", spellcheck: "false", "aria-label": "Body" });
  text.value = request.body;
  const send = async () => {
    try {
      state.selectWhenSeen = await call("compose_send", {
        request: { method: method.value, url: url.value.trim(), headers: parseHeaders(headers.value), body: text.value, note: null },
      });
    } catch {
      return;
    }
    $("compose-dialog").close();
    showInfo(`Mandado como #${state.selectWhenSeen}`);
  };
  $("compose-body").replaceChildren(
    el("h2", { id: "compose-title", text: fromId === undefined ? "Compose" : `Editar y repetir #${fromId}` }),
    request.note ? el("p", { class: "note", text: request.note }) : "",
    el("div", { class: "form" },
      el("label", { text: "Método" }), method,
      el("label", { text: "URL" }), url,
      el("label", { text: "Headers" }), headers,
      el("label", { text: "Body" }), text),
    el("p", { class: "hint", text: "Sale por el proxy: se captura y se le aplican las reglas. El proxy tiene que estar prendido." }),
    el("div", { class: "actions" }, el("button", { type: "button", class: "primary", onclick: send }, "Enviar")),
  );
  if (!$("compose-dialog").open) $("compose-dialog").showModal();
  url.focus();
}

// ---------- Arranque ----------

function bind() {
  $("toggle").addEventListener("click", toggleProxy);
  $("har").addEventListener("click", exportHar);
  $("filter").addEventListener("input", renderAll);
  $("clear").addEventListener("click", () => call("clear_flows").catch(() => {}));
  $("cert").addEventListener("click", () => {
    $("cert-dialog").showModal();
    showCert("this");
  });
  $("cert-close").addEventListener("click", () => $("cert-dialog").close());
  $("rules").addEventListener("click", () => openRules().catch(() => {}));
  $("rules-close").addEventListener("click", () => $("rules-dialog").close());
  $("compose").addEventListener("click", () => openCompose());
  $("compose-close").addEventListener("click", () => $("compose-dialog").close());
  $("paused").addEventListener("click", () => {
    $("bp-dialog").showModal();
    loadPaused().catch(() => {});
  });
  $("bp-close").addEventListener("click", () => $("bp-dialog").close());
  document.addEventListener("click", (e) => {
    if (!$("menu").hidden && !$("menu").contains(e.target)) closeMenu();
  });
  window.addEventListener("blur", closeMenu);
  for (const b of document.querySelectorAll(".dlg-nav button")) b.addEventListener("click", () => showCert(b.dataset.target));
  for (const tab of document.querySelectorAll(".tabs button")) {
    tab.addEventListener("click", () => {
      state.side = tab.dataset.side;
      refreshDetail();
    });
  }
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !$("menu").hidden) { closeMenu(); return; }
    if (["INPUT", "TEXTAREA", "SELECT"].includes(e.target.tagName) || document.querySelector("dialog[open]")) return;
    if (e.key === "ArrowDown") { e.preventDefault(); moveSelection(1); }
    if (e.key === "ArrowUp") { e.preventDefault(); moveSelection(-1); }
  });
}

async function main() {
  bind();
  // Escuchar antes de pedir la lista: lo que llegue en el medio se aplica igual (upsert es idempotente).
  await listen("proxyrr://notice", ({ payload }) => {
    switch (payload.type) {
      case "flow":
        upsert(payload.flow);
        break;
      case "cleared":
        state.flows.clear();
        state.selected = null;
        $("detail").hidden = true;
        $("no-selection").hidden = false;
        renderAll();
        break;
      case "proxy":
        setProxy(payload.proxy);
        break;
      case "lagged":
        resync().catch(() => {});
        refreshStatus().catch(() => {});
        break;
      case "log":
        showLog(payload.level, payload.message);
        break;
      default:
    }
  });
  const status = await call("status");
  setProxy(status.proxy);
  await resync();
  await refreshStatus();
  await listen("proxyrr://breakpoint", ({ payload }) => onBreakpoint(payload));
  await loadPaused();
}

main().catch((e) => showError(e));
