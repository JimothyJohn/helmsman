"use strict";
// Browser side of the PLC: token in sessionStorage, poll /api/state, edit and
// deploy the program. No framework, no build step.

const $ = (id) => document.getElementById(id);
const KEY = "tcpethernet-plc-token";
let token = "";
try { token = sessionStorage.getItem(KEY) || ""; } catch (_) { /* storage blocked */ }
// One-click login: http://plc:8080/#token=... (the fragment never reaches the
// server); stored for this tab, then scrubbed from the address bar.
const frag = new URLSearchParams(location.hash.slice(1)).get("token");
if (frag) {
  token = frag;
  try { sessionStorage.setItem(KEY, token); } catch (_) {}
  history.replaceState(null, "", location.pathname);
}
let deployed = "";
let errorLines = new Set();

async function api(method, path, body) {
  const res = await fetch(path, {
    method,
    headers: { Authorization: "Bearer " + token, "Content-Type": "text/plain" },
    body,
    cache: "no-store",
  });
  if (res.status === 401) { askToken("Token rejected."); throw new Error("unauthorized"); }
  return res;
}

function askToken(msg) {
  $("login-err").textContent = msg || "";
  if (!$("login").open) $("login").showModal();
}

$("login-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  token = $("token").value.trim();
  try { sessionStorage.setItem(KEY, token); } catch (_) {}
  $("login").close();
  await loadProgram();
});

function renderGutter() {
  const n = $("src").value.split("\n").length;
  const out = [];
  for (let i = 1; i <= n; i++) out.push(errorLines.has(i) ? `<span class="bad">${i}</span>` : String(i));
  $("gutter").innerHTML = out.join("\n");
  $("gutter").scrollTop = $("src").scrollTop;
  $("dirty").textContent = $("src").value === deployed ? "" : "unsaved changes";
}

function showErrors(errors) {
  const ul = $("errors");
  ul.replaceChildren();
  errorLines = new Set(errors.map((e) => e.line));
  if (!errors.length) {
    const li = document.createElement("li");
    li.className = "ok";
    li.textContent = "Compiled and running.";
    ul.append(li);
  }
  for (const e of errors) {
    const li = document.createElement("li");
    li.textContent = `line ${e.line}, col ${e.col}: ${e.msg}`;
    li.addEventListener("click", () => jumpTo(e.line, e.col));
    ul.append(li);
  }
  renderGutter();
}

function jumpTo(line, col) {
  const lines = $("src").value.split("\n");
  let pos = 0;
  for (let i = 0; i < line - 1 && i < lines.length; i++) pos += lines[i].length + 1;
  pos += Math.max(0, col - 1);
  $("src").focus();
  $("src").setSelectionRange(pos, pos);
}

async function loadProgram() {
  const res = await api("GET", "/api/program");
  deployed = await res.text();
  $("src").value = deployed;
  errorLines = new Set();
  $("errors").replaceChildren();
  renderGutter();
}

async function deploy() {
  const src = $("src").value;
  const res = await api("PUT", "/api/program", src);
  const body = await res.json();
  if (body.ok) { deployed = src; showErrors([]); } else showErrors(body.errors || []);
}

function cell(v, i) {
  const d = document.createElement("div");
  if (v) d.className = "nz";
  d.title = `byte ${i} = ${v} = 0b${v.toString(2).padStart(8, "0")}`;
  d.innerHTML = `${v.toString(16).padStart(2, "0")}<small>${i}</small>`;
  return d;
}

function bytes(el, arr, editable) {
  el.replaceChildren(...arr.map((v, i) => {
    const d = cell(v, i);
    if (editable) d.addEventListener("click", async () => {
      const s = prompt(`M[${i}] (0-255)`, String(v));
      if (s === null) return;
      const res = await api("PUT", `/api/memory/${i}`, s.trim());
      if (!res.ok) alert((await res.json()).error || "rejected");
    });
    return d;
  }));
}

function ms(n) { return n < 1000 ? `${n} ms` : n < 60000 ? `${(n / 1000).toFixed(1)} s` : `${Math.round(n / 60000)} min`; }

async function poll() {
  if (!token) return askToken();
  try {
    const s = await (await api("GET", "/api/state")).json();
    $("name").textContent = s.identity.name;
    $("ident").textContent = `vendor ${s.identity.vendor_id} · type ${s.identity.device_type} · product ${s.identity.product_code} · rev ${s.identity.revision} · serial ${s.identity.serial}`;
    const run = $("run");
    run.textContent = s.run ? "scanner: RUN" : s.connections.length ? "scanner: IDLE" : "no scanner";
    run.className = "pill " + (s.run ? "run" : "idle");
    $("conns").textContent = `${s.connections.length} connection${s.connections.length === 1 ? "" : "s"} · ${s.adapter.sessions} session${s.adapter.sessions === 1 ? "" : "s"}`;
    $("scan").textContent = `scan ${s.scan.last_us} µs (max ${s.scan.max_us}) / ${s.scan.period_us / 1000} ms`;
    $("i-asm").textContent = `assembly ${s.input_assembly}, from the scanner`;
    $("q-asm").textContent = `assembly ${s.output_assembly}, to the scanner`;
    bytes($("i"), s.i, false);
    bytes($("q"), s.q, false);
    bytes($("m"), s.m, true);
    const tb = $("conn-table").querySelector("tbody");
    tb.replaceChildren(...s.connections.map((c) => {
      const tr = document.createElement("tr");
      for (const t of [c.kind, c.originator, `${c.rpi_us / 1000} ms`, `${c.packets_in} / ${c.packets_out}`, ms(c.age_ms)]) {
        const td = document.createElement("td"); td.textContent = t; tr.append(td);
      }
      return tr;
    }));
    $("stats").textContent = `${s.scan.count} scans · ${s.scan.faults} faults · ${s.scan.overruns} overruns · ` +
      `${s.adapter.requests} requests · ${s.adapter.forward_opens} opens · ${s.adapter.refusals} refused · ` +
      `${s.adapter.timeouts} timeouts · ${s.adapter.io_dropped} I/O dropped · up ${ms(s.uptime_ms)}`;
  } catch (_) { /* next poll retries */ }
}

$("src").addEventListener("input", renderGutter);
$("src").addEventListener("scroll", () => { $("gutter").scrollTop = $("src").scrollTop; });
$("src").addEventListener("keydown", (e) => {
  if (e.key === "Tab") {
    e.preventDefault();
    const t = e.target, s = t.selectionStart;
    t.setRangeText("    ", s, t.selectionEnd, "end");
    renderGutter();
  } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); deploy(); }
});
$("deploy").addEventListener("click", deploy);
$("revert").addEventListener("click", () => { $("src").value = deployed; renderGutter(); });

if (token) loadProgram().catch(() => {}); else askToken();
setInterval(poll, 250);
poll();
