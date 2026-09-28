// UI de ProxyRR. Todo lo que viene de un flujo se inserta como texto (`textContent`), nunca como HTML:
// un header o un body capturado no puede inyectar nada en la ventana.
import {
  FlowList,
  base64ToBytes,
  formatMs,
  formatSize,
  hexDump,
  hostOf,
  matchesFilter,
  parseList,
  portOf,
  prettyJson,
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
  tr.replaceChildren(
    el("td", { class: "num", text: f.id }),
    el("td", { text: f.method }),
    el("td", { class: statusClass(f), text: f.in_progress ? "…" : f.status }),
    el("td", { text: f.url, title: f.url }),
    el("td", { class: "num", text: formatSize(f.response_size) }),
    el("td", { class: "num", text: formatMs(f.elapsed_ms) }),
  );
}

function makeRow(f) {
  const tr = el("tr", { "data-id": f.id, onclick: () => select(f.id) });
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
    body.replaceChildren(...(target === "this" ? await thisComputer() : await deviceGuide(target)));
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

// ---------- Arranque ----------

function bind() {
  $("toggle").addEventListener("click", toggleProxy);
  $("filter").addEventListener("input", renderAll);
  $("clear").addEventListener("click", () => call("clear_flows").catch(() => {}));
  $("cert").addEventListener("click", () => {
    $("cert-dialog").showModal();
    showCert("this");
  });
  $("cert-close").addEventListener("click", () => $("cert-dialog").close());
  for (const b of document.querySelectorAll(".dlg-nav button")) b.addEventListener("click", () => showCert(b.dataset.target));
  for (const tab of document.querySelectorAll(".tabs button")) {
    tab.addEventListener("click", () => {
      state.side = tab.dataset.side;
      refreshDetail();
    });
  }
  document.addEventListener("keydown", (e) => {
    if (e.target.tagName === "INPUT" || $("cert-dialog").open) return;
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
      default:
    }
  });
  const status = await call("status");
  setProxy(status.proxy);
  await resync();
  await refreshStatus();
}

main().catch((e) => showError(e));
