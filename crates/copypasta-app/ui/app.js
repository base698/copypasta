// Copypasta panel: search list on the left, selected clip + lineage DAG on the right.

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);

const $q = document.getElementById("q");
const $list = document.getElementById("list");
const $detail = document.getElementById("detail");
const $status = document.getElementById("status");

let rows = [];
let sel = 0;
let selectedId = null;
let searchTimer = null;
const thumbs = new Map();

const PLACE_COLOR = { herdr: "var(--herdr)", slack: "var(--slack)", web: "var(--web)", app: "var(--app)", system: "var(--system)" };
const KIND_BADGE = { text: "Aa", url: "↗", image: "▣", file: "📄" };
const CONTENT_LINKS = new Set(["contains", "excerpt", "similar", "image_similar", "same_url"]);
const LINK_TEXT = {
  contains: "quotes", excerpt: "excerpt of", similar: "similar to", image_similar: "same image",
  same_url: "from URL", via_context: "answer",
};

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

function ago(ms) {
  const d = (Date.now() - ms) / 1000;
  if (d < 45) return "just now";
  if (d < 3600) return `${Math.round(d / 60)}m`;
  if (d < 86400) return `${Math.round(d / 3600)}h`;
  const dt = new Date(ms);
  if (d < 6 * 86400) return dt.toLocaleDateString(undefined, { weekday: "short" }) + " " + hhmm(ms);
  return dt.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
const hhmm = (ms) => new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });

function chip(place, dst) {
  const c = PLACE_COLOR[place.kind] || PLACE_COLOR.app;
  return `<span class="chip${dst ? " dst" : ""}" style="--c:${c}" title="${esc((dst ? "pasted into " : "copied from ") + place.label)}">${dst ? "→ " : ""}${esc(place.label)}</span>`;
}

async function refresh(keepSelection = true) {
  try {
    rows = await invoke("search", { query: $q.value, limit: 200 });
  } catch (e) {
    console.error(e);
    rows = [];
  }
  if (keepSelection && selectedId != null) {
    const i = rows.findIndex((r) => r.id === selectedId);
    sel = i >= 0 ? i : 0;
  } else {
    sel = 0;
  }
  renderList();
  showSelected();
}

function renderList() {
  $list.innerHTML = rows
    .map((r, i) => {
      const badge = r.kind === "image" ? `<span class="kind" data-thumb="${r.id}">▣</span>` : `<span class="kind">${KIND_BADGE[r.kind] || "Aa"}</span>`;
      const src = r.sources.slice(0, 1).map((p) => chip(p, false)).join("");
      const dst = r.destinations.slice(0, 2).map((p) => chip(p, true)).join("");
      const more = r.destinations.length > 2 ? `<span class="chip dst">+${r.destinations.length - 2}</span>` : "";
      const mono = /[{};=<>]|^\s*(\$|git |cargo |npm )/.test(r.preview) && r.kind === "text";
      return `<li data-i="${i}" class="${i === sel ? "sel" : ""}" role="option">
        ${badge}
        <div class="body">
          <div class="preview${mono ? " mono" : ""}">${esc(r.preview)}</div>
          <div class="meta">${r.pinned ? '<span class="pin">★</span>' : ""}${src}${dst}${more}<span class="time">${ago(r.last_seen)}</span></div>
        </div></li>`;
    })
    .join("");
  if (!rows.length) {
    $list.innerHTML = `<li class="empty" style="display:block">${$q.value ? "No matches." : "No clips yet."}</li>`;
  }
  $list.querySelectorAll("[data-thumb]").forEach(loadThumb);
}

async function loadThumb(el) {
  const id = Number(el.dataset.thumb);
  if (!thumbs.has(id)) thumbs.set(id, invoke("image", { id, max: 96 }).catch(() => null));
  const url = await thumbs.get(id);
  if (url) el.innerHTML = `<img src="${url}" alt="">`;
}

function select(i) {
  if (!rows.length) return;
  sel = Math.max(0, Math.min(rows.length - 1, i));
  $list.querySelectorAll("li").forEach((li) => li.classList.toggle("sel", Number(li.dataset.i) === sel));
  $list.querySelector("li.sel")?.scrollIntoView({ block: "nearest" });
  showSelected();
}

let detailSeq = 0;
async function showSelected() {
  const r = rows[sel];
  if (!r) {
    selectedId = null;
    $detail.innerHTML = `<div class="empty">${$q.value ? "No clips match that search." : "Nothing copied yet. Copy something and it shows up here."}</div>`;
    return;
  }
  selectedId = r.id;
  const seq = ++detailSeq;
  const [graph, img] = await Promise.all([
    invoke("graph", { id: r.id }).catch(() => null),
    r.kind === "image" ? invoke("image", { id: r.id, max: 900 }).catch(() => null) : null,
  ]);
  if (seq !== detailSeq) return;
  let content;
  if (r.kind === "image") {
    content = `${img ? `<img src="${img}" alt="">` : "Image unavailable"}<div class="src">${esc(r.width)}×${esc(r.height)}${r.text ? " · " + esc(r.text) : ""}</div>`;
  } else if (r.kind === "url" && /\.(png|jpe?g|gif|webp|svg)(\?|$)/i.test(r.text || "")) {
    content = `<img src="${esc(r.text)}" alt=""><div class="src">${esc(r.text)}</div>`;
  } else {
    content = `<pre>${esc(r.secret ? r.preview : r.text ?? r.preview)}</pre>`;
  }
  const facts = `copied ${r.copies}× · pasted ${r.pastes}× · first ${ago(r.first_seen)}`;
  $detail.innerHTML = `
    <div class="content">${content}</div>
    <div class="actions">
      <button class="primary" data-act="copy" ${r.secret ? "disabled" : ""}>Copy ⏎</button>
      <button data-act="paste" ${r.secret ? "disabled" : ""}>Paste ⌘⏎</button>
      <button data-act="pin">${r.pinned ? "Unpin" : "Pin"}</button>
      <button data-act="delete">Delete</button>
      <span class="facts">${esc(facts)}</span>
    </div>
    <div class="dagwrap"><h3>Copy / paste path</h3><div id="dag"></div>
      <div class="legend"><span><i></i>pasted / re-copied</span><span><i class="c"></i>content link</span><span><i class="w"></i>copied out of an assistant after a paste</span></div>
    </div>`;
  if (graph) renderDag(document.getElementById("dag"), graph);
}

// ---- DAG layout: time flows down. Row = longest path from a root; each row
// is ordered by time and centered.
const NW = 236, NH = 66, GX = 18, GY = 46, PAD = 16;

function nodeTitle(n) {
  const h = n.ctx.herdr;
  if (h) {
    const who = h.agent || h.pane_label || "shell";
    const tab = h.tab && !/^\d+$/.test(h.tab) ? ` · ${h.tab}` : "";
    return `herdr ${who}${tab}`;
  }
  return n.label;
}

function nodeDetail(n) {
  const h = n.ctx.herdr;
  if (h) return [h.cwd ? h.cwd.replace(/^\/Users\/[^/]+/, "~") : null, h.title].filter(Boolean).join(" · ");
  return n.detail || "";
}

function renderDag(el, g) {
  if (!g.nodes.length) {
    el.innerHTML = `<div class="empty">No copy or paste events recorded for this clip yet.</div>`;
    return;
  }
  const byId = new Map(g.nodes.map((n) => [n.id, n]));
  const parents = new Map(g.nodes.map((n) => [n.id, []]));
  for (const e of g.edges) if (byId.has(e.from) && byId.has(e.to)) parents.get(e.to).push(e.from);
  const layer = new Map();
  const depth = (id, seen = new Set()) => {
    if (layer.has(id)) return layer.get(id);
    if (seen.has(id)) return 0;
    seen.add(id);
    const ps = parents.get(id) || [];
    const d = ps.length ? Math.max(...ps.map((p) => depth(p, seen))) + 1 : 0;
    layer.set(id, d);
    return d;
  };
  g.nodes.forEach((n) => depth(n.id));
  const rowsOf = [];
  for (const n of g.nodes) (rowsOf[layer.get(n.id)] ||= []).push(n);
  const widest = Math.max(...rowsOf.map((r) => r.length));
  const avail = Math.max(el.clientWidth - PAD * 2, NW);
  const W = Math.max(PAD * 2 + widest * (NW + GX) - GX, avail + PAD * 2);
  const pos = new Map();
  rowsOf.forEach((row, y) => {
    row.sort((a, b) => a.ts - b.ts);
    const rowW = row.length * (NW + GX) - GX;
    const x0 = (W - rowW) / 2;
    row.forEach((n, i) => pos.set(n.id, { x: x0 + i * (NW + GX), y: PAD + y * (NH + GY) }));
  });
  const H = PAD * 2 + rowsOf.length * (NH + GY) - GY;
  const clipPreview = new Map(g.clips.map((c) => [c.id, c.preview]));

  let svg = `<svg class="dag" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" xmlns="http://www.w3.org/2000/svg">`;
  for (const e of g.edges) {
    const a = pos.get(e.from), b = pos.get(e.to);
    if (!a || !b) continue;
    const x1 = a.x + NW / 2, y1 = a.y + NH, x2 = b.x + NW / 2, y2 = b.y;
    const my = (y1 + y2) / 2;
    const cls = ["edge", e.weak ? "weak" : "", CONTENT_LINKS.has(e.kind) ? "content" : ""].join(" ");
    svg += `<path class="${cls}" d="M${x1},${y1} C${x1},${my} ${x2},${my} ${x2},${y2}"><title>${esc(LINK_TEXT[e.kind] || e.kind)}</title></path>`;
    if (LINK_TEXT[e.kind]) svg += `<text class="elabel" x="${(x1 + x2) / 2 + 6}" y="${my + 3}">${esc(LINK_TEXT[e.kind])}</text>`;
  }
  for (const n of g.nodes) {
    const p = pos.get(n.id);
    const other = n.clip_id !== g.focus;
    const color = PLACE_COLOR[n.place] || PLACE_COLOR.app;
    const title = other ? `clip #${n.clip_id}: ${clipPreview.get(n.clip_id) || ""}` : n.label;
    const det = nodeDetail(n);
    svg += `<g class="node${other ? " other" : " focus"}" data-clip="${n.clip_id}" transform="translate(${p.x},${p.y})">
      <title>${esc(title)}${det ? "\n" + esc(det) : ""}</title>
      <rect class="box" width="${NW}" height="${NH}" rx="9"/>
      <rect width="4" height="${NH - 16}" x="0" y="8" rx="2" fill="${color}"/>
      <text class="verb ${n.kind}" x="14" y="18">${n.kind === "copy" ? "COPIED FROM" : "PASTED INTO"}</text>
      <text class="ts" x="${NW - 10}" y="18" text-anchor="end">${esc(hhmm(n.ts))}${other ? " · #" + n.clip_id : ""}</text>
      <text class="label" x="14" y="37">${esc(fit(nodeTitle(n), 31))}</text>
      <text class="det" x="14" y="54">${esc(fit(det, 40))}</text>
    </g>`;
  }
  svg += "</svg>";
  el.innerHTML = svg;
  el.querySelectorAll(".node.other").forEach((node) =>
    node.addEventListener("click", () => focusClip(Number(node.dataset.clip))),
  );
  el.querySelector(".node.focus")?.scrollIntoView({ block: "nearest", inline: "center" });
}

const fit = (s, n) => (s.length > n ? s.slice(0, n - 1) + "…" : s);

async function focusClip(id) {
  let i = rows.findIndex((r) => r.id === id);
  if (i < 0) {
    $q.value = "";
    await refresh(false);
    i = rows.findIndex((r) => r.id === id);
  }
  if (i >= 0) select(i);
}

async function act(what) {
  const r = rows[sel];
  if (!r) return;
  if (what === "copy" || what === "paste") {
    try {
      await invoke("restore", { id: r.id, paste: what === "paste" });
    } catch (e) {
      flash(String(e));
    }
  } else if (what === "pin") {
    await invoke("pin", { id: r.id, pinned: !r.pinned });
    refresh();
  } else if (what === "delete") {
    await invoke("delete", { id: r.id });
    selectedId = rows[sel + 1]?.id ?? rows[sel - 1]?.id ?? null;
    refresh();
  }
}

function flash(msg) {
  $status.innerHTML = `<span class="warn">${esc(msg)}</span>`;
  setTimeout(renderStatus, 3000);
}

async function renderStatus() {
  try {
    const s = await invoke("status");
    const warn = [];
    if (!s.paste_tracking) warn.push("paste tracking off");
    if (!s.accessibility) warn.push("no window titles");
    $status.innerHTML =
      (warn.length ? `<span class="warn">${warn.join(" · ")}</span><button id="grant">Grant access…</button>` : "") +
      `<span>${s.stats.clips} clips · ${s.stats.pastes} pastes</span>`;
    document.getElementById("grant")?.addEventListener("click", () => invoke("request_permissions"));
  } catch (e) {
    console.error(e);
  }
}

// ---- events
$q.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => refresh(false), 70);
});

document.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") { e.preventDefault(); select(sel + 1); }
  else if (e.key === "ArrowUp") { e.preventDefault(); select(sel - 1); }
  else if (e.key === "PageDown") { e.preventDefault(); select(sel + 10); }
  else if (e.key === "PageUp") { e.preventDefault(); select(sel - 10); }
  else if (e.key === "Enter") { e.preventDefault(); act(e.metaKey ? "paste" : "copy"); }
  else if (e.key === "Escape") {
    e.preventDefault();
    if ($q.value) { $q.value = ""; refresh(false); } else invoke("hide");
  }
  else if (e.metaKey && e.key.toLowerCase() === "p") { e.preventDefault(); act("pin"); }
  else if (e.metaKey && e.key === "Backspace") { e.preventDefault(); act("delete"); }
  if (document.activeElement !== $q && !e.metaKey && e.key.length === 1) $q.focus();
});

$list.addEventListener("click", (e) => {
  const li = e.target.closest("li[data-i]");
  if (li) select(Number(li.dataset.i));
});
$list.addEventListener("dblclick", (e) => {
  if (e.target.closest("li[data-i]")) act("copy");
});
$detail.addEventListener("click", (e) => {
  const b = e.target.closest("button[data-act]");
  if (b) act(b.dataset.act);
});

let changeTimer = null;
T.event.listen("clips-changed", () => {
  clearTimeout(changeTimer);
  changeTimer = setTimeout(() => refresh(true), 150);
});
T.event.listen("panel-shown", () => {
  $q.focus();
  $q.select();
  selectedId = null;
  refresh(false);
  renderStatus();
});

$q.focus();
refresh(false);
renderStatus();
