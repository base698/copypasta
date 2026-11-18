// Copypasta panel: grouped, paged clip list on the left; the selected clip
// (or group) and its lineage DAG on the right.

const T = window.__TAURI__;
const invoke = (cmd, args) => T.core.invoke(cmd, args);
const call = (cmd, args = {}) => invoke("api", { cmd, args });

const $q = document.getElementById("q");
const $list = document.getElementById("list");
const $detail = document.getElementById("detail");
const $status = document.getElementById("status");

const PAGE = 40;
const PLACE_COLOR = { herdr: "var(--herdr)", slack: "var(--slack)", web: "var(--web)", app: "var(--app)", system: "var(--system)" };
const KIND_BADGE = { text: "Aa", url: "↗", image: "▣", file: "📄" };

let groups = [];          // summaries from `groups`, in display order
let allGroups = [];       // every group, for the "move to" menu
const lists = new Map();  // group key -> { rows, done, loading }
let openKeys = new Set(); // expanded group keys
let expanded = loadExpanded(); // remembered expansion when not searching
let pinnedClosed = loadFlag("copypasta.pinnedClosed");
let pinnedGroup = null;   // the "Pinned" section: pinned clips from every group
let entries = [];         // visible rows: {t:"g", key, g} | {t:"c", key, row}
let sel = 0;
let cursor = null;        // {t, key, id?} — survives re-renders
let query = "";
let refreshSeq = 0;
let searchTimer = null;
const thumbs = new Map();

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fit = (s, n) => (s.length > n ? s.slice(0, n - 1) + "…" : s);
const gkey = (g) => (g.pinned ? "pinned" : g.id == null ? "none" : String(g.id));
const gsel = (key) => (key === "none" || key === "pinned" ? key : Number(key));
const searching = () => query.split(/\s+/).some((t) => t && !/^(g|group):/i.test(t));

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

function loadExpanded() {
  try {
    const v = JSON.parse(localStorage.getItem("copypasta.expanded"));
    return Array.isArray(v) ? new Set(v) : null;
  } catch {
    return null;
  }
}
function loadFlag(k) {
  try {
    return localStorage.getItem(k) === "1";
  } catch {
    return false;
  }
}
function saveFlag(k, on) {
  try {
    localStorage.setItem(k, on ? "1" : "0");
  } catch {}
}
function saveExpanded() {
  if (searching()) return;
  expanded = new Set([...openKeys].filter((k) => k !== "pinned"));
  try {
    localStorage.setItem("copypasta.expanded", JSON.stringify([...expanded]));
  } catch {}
}

// ---- data

async function refresh({ keep = true } = {}) {
  query = $q.value.trim();
  const seq = ++refreshSeq;
  let gs, all, pins;
  try {
    [gs, all, pins] = await Promise.all([
      call("groups", { query }),
      call("all_groups"),
      call("search", { query, group: "pinned", limit: 500 }),
    ]);
  } catch (e) {
    console.error(e);
    gs = [];
    all = [];
    pins = [];
  }
  if (seq !== refreshSeq) return;
  groups = gs;
  allGroups = all;
  pinnedGroup = pins.length
    ? { pinned: true, id: null, name: "Pinned", count: pins.length, incoming: 0, last_seen: Math.max(...pins.map((r) => r.last_seen)) }
    : null;
  const keys = new Set(groups.map(gkey));
  if (searching() || /(^|\s)(g|group):/i.test(query)) {
    openKeys = new Set(groups.filter((g) => g.count + g.incoming > 0).map(gkey));
  } else {
    openKeys = new Set([...(expanded ?? new Set(groups.slice(0, 1).map(gkey)))].filter((k) => keys.has(k)));
  }
  if (pinnedGroup && (searching() || !pinnedClosed)) openKeys.add("pinned");
  const prev = new Map(lists);
  lists.clear();
  lists.set("pinned", { rows: pins, done: true, loading: false });
  await Promise.all(
    [...openKeys].map((k) => loadPage(k, keep ? Math.max(PAGE, prev.get(k)?.rows.length ?? 0) : PAGE)),
  );
  if (seq !== refreshSeq) return;
  if (!keep) cursor = null;
  render();
}

async function loadPage(key, limit = PAGE) {
  let l = lists.get(key);
  if (!l) lists.set(key, (l = { rows: [], done: false, loading: false }));
  if (l.done || l.loading) return;
  l.loading = true;
  try {
    const rows = await call("search", { query, group: gsel(key), offset: l.rows.length, limit });
    const seen = new Set(l.rows.map((r) => r.id));
    l.rows.push(...rows.filter((r) => !seen.has(r.id)));
    l.done = rows.length < limit;
  } catch (e) {
    console.error(e);
    l.done = true;
  } finally {
    l.loading = false;
  }
}

// ---- list

// herdr labels are "herdr <agent> · <tab> · <cwd>"; in the list keep the
// agent and tab, and name the workspace only when it isn't this group's.
function chipLabel(place, g) {
  if (place.kind !== "herdr") return place.label;
  const parts = place.label.replace(/^herdr /, "").split(" · ").filter((p) => !/^[~/]/.test(p));
  const ws = place.workspace && place.workspace !== g?.workspace ? ` @ ${place.workspace}` : "";
  return parts.join(" · ") + ws;
}

function chip(place, dst, g) {
  const c = PLACE_COLOR[place.kind] || PLACE_COLOR.app;
  return `<span class="chip${dst ? " dst" : ""}" style="--c:${c}" title="${esc((dst ? "pasted into " : "copied from ") + place.label + (place.workspace ? ` (workspace ${place.workspace})` : ""))}">${dst ? "→ " : ""}${esc(chipLabel(place, g))}</span>`;
}

function groupHeader(g, open, i) {
  if (g.pinned) {
    return `<li class="group pinned${open ? " open" : ""}" data-i="${i}" data-key="pinned">
    <span class="chev">${open ? "▾" : "▸"}</span>
    <span class="gname"><span class="pin">★</span> Pinned</span>
    <span class="gcount">${g.count}</span>
    <span class="time"></span>
  </li>`;
  }
  const tag = g.workspace ? `<span class="gtag" title="herdr workspace ${esc(g.workspace)}">herdr</span>` : "";
  const inc = g.incoming ? `<span class="gin" title="pasted in from other groups">+${g.incoming} in</span>` : "";
  return `<li class="group${open ? " open" : ""}" data-i="${i}" data-key="${gkey(g)}">
    <span class="chev">${open ? "▾" : "▸"}</span>
    <span class="gname">${esc(g.name)}</span>${tag}
    <span class="gcount">${g.count}</span>${inc}
    <span class="time">${g.count + g.incoming ? ago(g.last_seen) : ""}</span>
  </li>`;
}

function clipRow(r, g, i) {
  const badge = r.kind === "image" ? `<span class="kind" data-thumb="${r.id}">▣</span>` : `<span class="kind">${KIND_BADGE[r.kind] || "Aa"}</span>`;
  const src = r.sources.slice(0, 1).map((p) => chip(p, false, g)).join("");
  const dst = r.destinations.slice(0, 2).map((p) => chip(p, true, g)).join("");
  const more = r.destinations.length > 2 ? `<span class="chip dst">+${r.destinations.length - 2}</span>` : "";
  let cross = "";
  if (g.pinned) {
    cross = `<span class="chip grp" title="in group ${esc(r.group?.name ?? "none")}">${esc(r.group?.name ?? "ungrouped")}</span>`;
  } else if (r.incoming) {
    cross = `<span class="chip in" title="belongs to ${esc(r.group?.name ?? "no group")}; pasted into this workspace">⇢ from ${esc(r.group?.name ?? "ungrouped")}</span>`;
  } else {
    const others = r.workspaces.filter((w) => w !== g.workspace);
    if (g.workspace && others.length) cross = `<span class="chip in" title="also pasted into these herdr workspaces">↔ ${esc(others.join(", "))}</span>`;
  }
  if (r.edit_of) cross += `<span class="chip edit" title="near-identical to clip #${r.edit_of}, with changes">edit of #${r.edit_of}</span>`;
  const mono = /[{};=<>]|^\s*(\$|git |cargo |npm )/.test(r.preview) && r.kind === "text";
  return `<li class="clip${r.incoming ? " incoming" : ""}" data-i="${i}" data-id="${r.id}" draggable="true" role="option">
    ${badge}
    <div class="body">
      <div class="preview${mono ? " mono" : ""}">${esc(r.preview)}</div>
      <div class="meta"><button class="star${r.pinned ? " on" : ""}" data-star="${r.id}" title="${r.pinned ? "Unpin" : "Pin"} (⌘P)" tabindex="-1">${r.pinned ? "★" : "☆"}</button>${cross}${src}${dst}${more}<span class="time">${ago(r.last_seen)}</span></div>
    </div></li>`;
}

let moreObserver = null;
function render() {
  entries = [];
  let html = "";
  for (const g of pinnedGroup ? [pinnedGroup, ...groups] : groups) {
    const key = gkey(g);
    const open = openKeys.has(key);
    entries.push({ t: "g", key, g });
    html += groupHeader(g, open, entries.length - 1);
    if (!open) continue;
    const l = lists.get(key);
    for (const r of l?.rows ?? []) {
      entries.push({ t: "c", key, row: r, g });
      html += clipRow(r, g, entries.length - 1);
    }
    if (l && !l.done) html += `<li class="more" data-more="${key}">Loading more…</li>`;
    else if (l && !l.rows.length && !g.pinned) html += `<li class="hint">${g.id == null ? "Nothing ungrouped." : "Empty. Drag clips here."}</li>`;
  }
  if (!groups.length) html = `<li class="hint">${query ? "No matches." : "No clips yet. Copy something."}</li>`;
  const top = $list.scrollTop;
  $list.innerHTML = html;
  $list.scrollTop = top;
  $list.querySelectorAll("[data-thumb]").forEach(loadThumb);

  moreObserver?.disconnect();
  moreObserver = new IntersectionObserver((obs) => {
    for (const o of obs) {
      if (!o.isIntersecting) continue;
      const key = o.target.dataset.more;
      const l = lists.get(key);
      if (l && !l.loading && !l.done) loadPage(key).then(render);
    }
  }, { root: $list, rootMargin: "200px" });
  $list.querySelectorAll("[data-more]").forEach((el) => moreObserver.observe(el));
  requestAnimationFrame(checkMore);

  let i = -1;
  if (cursor) {
    i = entries.findIndex((e) => e.t === cursor.t && e.key === cursor.key && (e.t === "g" || e.row.id === cursor.id));
    if (i < 0 && cursor.t === "c") i = entries.findIndex((e) => e.t === "c" && e.row.id === cursor.id);
    if (i < 0 && cursor.t === "c") i = entries.findIndex((e) => e.t === "g" && e.key === cursor.key);
  }
  // Default to the newest clip, not a pin: open-and-⏎ should get what you just copied.
  if (i < 0) i = entries.findIndex((e) => e.t === "c" && e.key !== "pinned");
  if (i < 0) i = entries.findIndex((e) => e.t === "c");
  if (i < 0) i = 0;
  select(i, { scroll: !cursor });
}

// Backup for the observer (which only fires while the page is rendering):
// load the next page of any group whose "more" row is near the viewport.
function checkMore() {
  const bottom = $list.getBoundingClientRect().bottom + 200;
  for (const el of $list.querySelectorAll("[data-more]")) {
    if (el.getBoundingClientRect().top > bottom) continue;
    const key = el.dataset.more;
    const l = lists.get(key);
    if (l && !l.loading && !l.done) loadPage(key).then(render);
  }
}
let scrollTimer = null;
$list.addEventListener("scroll", () => {
  clearTimeout(scrollTimer);
  scrollTimer = setTimeout(checkMore, 60);
});

async function loadThumb(el) {
  const id = Number(el.dataset.thumb);
  if (!thumbs.has(id)) thumbs.set(id, call("image", { id, max: 96 }).catch(() => null));
  const url = await thumbs.get(id);
  if (url) el.innerHTML = `<img src="${url}" alt="">`;
}

let shownKey = null;
function select(i, { scroll = true } = {}) {
  if (!entries.length) {
    cursor = null;
    showEmpty();
    return;
  }
  sel = Math.max(0, Math.min(entries.length - 1, i));
  const e = entries[sel];
  cursor = e.t === "g" ? { t: "g", key: e.key } : { t: "c", key: e.key, id: e.row.id };
  $list.querySelectorAll("li[data-i]").forEach((li) => li.classList.toggle("sel", Number(li.dataset.i) === sel));
  if (scroll) $list.querySelector("li.sel")?.scrollIntoView({ block: "nearest" });
  const showKey = e.t === "g" ? `g:${e.key}:${e.g.count}:${e.g.name}` : `c:${e.row.id}:${e.row.group?.id}:${e.row.pinned}:${e.row.last_seen}`;
  if (showKey !== shownKey) {
    shownKey = showKey;
    if (e.t === "g") showGroup(e.g);
    else showClip(e.row);
  }
  // Prefetch the next page as the cursor nears the end of a group.
  if (e.t === "c") {
    const l = lists.get(e.key);
    const idx = l?.rows.findIndex((r) => r.id === e.row.id) ?? -1;
    if (l && !l.done && !l.loading && idx >= l.rows.length - 5) loadPage(e.key).then(render);
  }
}

function toggle(key, open = !openKeys.has(key)) {
  if (open) openKeys.add(key);
  else openKeys.delete(key);
  if (key === "pinned" && !searching()) saveFlag("copypasta.pinnedClosed", (pinnedClosed = !open));
  saveExpanded();
  if (open && !lists.has(key)) loadPage(key).then(render);
  else render();
}

// ---- detail

function showEmpty() {
  shownKey = null;
  $detail.innerHTML = `<div class="empty">${query ? "No clips match that search." : "Nothing copied yet. Copy something and it shows up here."}</div>`;
}

function groupOptions(r) {
  const cur = r.group?.id;
  let o = "";
  if (r.group?.manual || !r.group) o += `<option value="auto">Automatic (herdr workspace)</option>`;
  for (const g of allGroups) o += `<option value="${g.id}" ${g.id === cur ? "selected" : ""}>${esc(g.name)}${g.workspace ? " · herdr" : ""}</option>`;
  o += `<option value="none" ${cur == null ? "selected" : ""}>No group</option>`;
  o += `<option value="new">New group…</option>`;
  return o;
}

let detailSeq = 0;
async function showClip(r) {
  const seq = ++detailSeq;
  const [graph, img] = await Promise.all([
    call("graph", { id: r.id }).catch(() => null),
    r.kind === "image" ? call("image", { id: r.id, max: 900 }).catch(() => null) : null,
  ]);
  if (seq !== detailSeq) return;
  let content;
  if (r.kind === "image") {
    const file = r.path ? " · " + esc(r.path.replace(/^\/Users\/[^/]+/, "~")) : "";
    content = `${img ? `<img src="${img}" alt="">` : "Image unavailable"}<div class="src">${esc(r.width)}×${esc(r.height)}${r.text ? " · " + esc(r.text) : ""}${file}</div>`;
  } else if (r.kind === "url" && /\.(png|jpe?g|gif|webp|svg)(\?|$)/i.test(r.text || "")) {
    content = `<img src="${esc(r.text)}" alt=""><div class="src">${esc(r.text)}</div>`;
  } else {
    content = `<pre>${esc(r.secret ? r.preview : r.text ?? r.preview)}</pre>`;
  }
  const facts = `copied ${r.copies}× · pasted ${r.pastes}× · first ${ago(r.first_seen)}`;
  const crossed = r.workspaces.length > 1 ? `<span class="crossed" title="herdr workspaces this clip moved between">↔ ${esc(r.workspaces.join(" → "))}</span>` : "";
  $detail.innerHTML = `
    <div class="content">${content}</div>
    <div class="actions">
      <button class="primary" data-act="copy" ${r.secret ? "disabled" : ""}>Copy ⏎</button>
      <button data-act="paste" ${r.secret ? "disabled" : ""}>Paste ⌘⏎</button>
      ${r.kind === "image" || r.kind === "file" ? '<button data-act="path" title="Paste the path of the file instead of the image">Paste path ⇧⌘⏎</button>' : ""}
      <button data-act="pin">${r.pinned ? "Unpin" : "Pin"}</button>
      <button data-act="delete">Delete</button>
      <span class="facts">${esc(facts)}</span>
    </div>
    <div class="actions grouprow">
      <label>Group <select id="grp" title="⌘G">${groupOptions(r)}</select></label>
      ${r.group?.manual ? '<span class="facts">set by you</span>' : r.group ? '<span class="facts">automatic</span>' : ""}
      <span id="newgrp" hidden><input id="newname" placeholder="New group name" maxlength="80"><button data-act="create">Create</button></span>
      ${crossed}
    </div>
    <div class="dagwrap"><h3>Where it came from and went</h3>
      <p class="hint">Oldest first. Each card is one clip; ↳ says how it relates to an earlier one. Click a card to open it.</p>
      <div id="dag"></div>
    </div>`;
  document.getElementById("grp").addEventListener("change", onGroupChange);
  if (graph) renderDag(document.getElementById("dag"), graph);
}

function showGroup(g) {
  detailSeq++;
  if (g.pinned) {
    $detail.innerHTML = `
    <div class="groupinfo">
      <h2><span class="pin">★</span> Pinned</h2>
      <p class="muted">${g.count} clip${g.count === 1 ? "" : "s"}${query ? " matching" : ""}</p>
      <p>Clips you pinned, from every group. They also stay in their own group, at the top, and are never cleaned up with old history.</p>
      <p class="muted small">Pin or unpin with <kbd>⌘P</kbd>, the ☆ on a row, or by dragging a clip onto this header · search <code>is:pinned</code> to filter</p>
    </div>`;
    return;
  }
  const desc = g.id == null
    ? "Clips copied outside herdr that haven't been pasted into a workspace yet, plus anything you took out of a group."
    : g.workspace
      ? `Automatic group for the herdr workspace <b>${esc(g.workspace)}</b>: clips first copied there, and clips from outside herdr first pasted there.`
      : "A group you made. Drag clips onto it, or pick it from a clip's Group menu.";
  const inc = g.incoming ? `<p>${g.incoming} clip${g.incoming === 1 ? "" : "s"} from other groups were pasted into this workspace. They're listed here with ⇢ but still belong to their own group.</p>` : "";
  $detail.innerHTML = `
    <div class="groupinfo">
      <h2>${esc(g.name)}</h2>
      <p class="muted">${g.count} clip${g.count === 1 ? "" : "s"}${query ? " matching" : ""}${g.count + g.incoming ? " · last active " + ago(g.last_seen) : ""}</p>
      <p>${desc}</p>${inc}
      ${g.id == null ? "" : `
      <div class="actions plain">
        <input id="rename" value="${esc(g.name)}" maxlength="80" aria-label="Group name">
        <button data-act="rename">Rename</button>
        <button data-act="delgroup" class="danger">Delete group</button>
      </div>
      <p class="muted small">Deleting a group keeps its clips; they become ungrouped.${g.workspace ? " New copies in this workspace will start a fresh group." : ""}</p>`}
      <p class="muted small"><kbd>⏎</kbd> or <kbd>→</kbd>/<kbd>←</kbd> expand and collapse · search <code>g:${esc(g.name.split(/\s+/)[0].toLowerCase())}</code> to filter</p>
    </div>`;
}

async function onGroupChange(ev) {
  const e = entries[sel];
  if (e?.t !== "c") return;
  const v = ev.target.value;
  if (v === "new") {
    document.getElementById("newgrp").hidden = false;
    document.getElementById("newname").focus();
    return;
  }
  try {
    if (v === "auto") await call("reset_group", { id: e.row.id });
    else await call("move", { id: e.row.id, group: v === "none" ? null : Number(v) });
  } catch (err) {
    flash(String(err));
  }
  await afterMove(e.row.id);
}

async function afterMove(id) {
  // Follow the clip into its new group.
  const row = await call("clip", { id }).catch(() => null);
  const key = row?.group ? String(row.group.id) : "none";
  openKeys.add(key);
  saveExpanded();
  cursor = { t: "c", key, id };
  shownKey = null;
  await refresh({ keep: true });
}

async function focusClip(id) {
  let i = entries.findIndex((e) => e.t === "c" && e.row.id === id);
  if (i < 0) {
    const row = await call("clip", { id }).catch(() => null);
    if (!row) return;
    const key = row.group ? String(row.group.id) : "none";
    if ($q.value) $q.value = "";
    query = "";
    openKeys = new Set([...(expanded ?? []), key]);
    saveExpanded();
    cursor = { t: "c", key, id };
    await refresh({ keep: true });
    for (let n = 0; n < 25 && !entries.some((e) => e.t === "c" && e.row.id === id) && !lists.get(key)?.done; n++) {
      await loadPage(key);
      render();
    }
    i = entries.findIndex((e) => e.t === "c" && e.row.id === id);
  }
  if (i >= 0) select(i);
}
window.focusClip = focusClip;

async function act(what) {
  const e = entries[sel];
  if (!e) return;
  if (e.t === "g") {
    const g = e.g;
    if (what === "rename") {
      const name = document.getElementById("rename").value.trim();
      if (name && name !== g.name) {
        await call("rename_group", { id: g.id, name }).catch((err) => flash(String(err)));
        shownKey = null;
        refresh();
      }
    } else if (what === "delgroup") {
      const b = $detail.querySelector('[data-act="delgroup"]');
      if (!b.dataset.armed) {
        b.dataset.armed = "1";
        b.textContent = "Click again to delete";
        return;
      }
      await call("delete_group", { id: g.id });
      cursor = null;
      refresh();
    } else if (what === "copy") {
      toggle(e.key);
    }
    return;
  }
  const r = e.row;
  if (what === "copy" || what === "paste" || what === "path") {
    try {
      // "path" pastes the clip's file path where it has one (images, files).
      await invoke("restore", { id: r.id, paste: what !== "copy", asPath: what === "path" });
    } catch (err) {
      flash(String(err));
    }
  } else if (what === "pin") {
    await togglePin(r);
  } else if (what === "delete") {
    await call("delete", { id: r.id });
    const next = entries[sel + 1]?.t === "c" ? entries[sel + 1] : entries[sel - 1];
    cursor = next ? (next.t === "g" ? { t: "g", key: next.key } : { t: "c", key: next.key, id: next.row.id }) : null;
    refresh();
  } else if (what === "create") {
    const name = document.getElementById("newname").value.trim();
    if (!name) return;
    try {
      await call("create_group", { name, ids: [r.id] });
    } catch (err) {
      flash(String(err));
    }
    await afterMove(r.id);
  }
}

async function togglePin(r) {
  try {
    await call("pin", { id: r.id, pinned: !r.pinned });
  } catch (err) {
    flash(String(err));
  }
  shownKey = null;
  await refresh();
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
      `<span>${s.stats.clips} clips · ${s.stats.groups ?? 0} groups</span>`;
    document.getElementById("grant")?.addEventListener("click", () => invoke("request_permissions"));
  } catch (e) {
    console.error(e);
  }
}

// ---- events

$q.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => refresh({ keep: false }), 80);
});

const inField = (el) => el && el !== $q && /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName);

document.addEventListener("keydown", (e) => {
  const a = document.activeElement;
  if (inField(a)) {
    if (e.key === "Escape") {
      e.preventDefault();
      a.blur();
      $q.focus();
    } else if (e.key === "Enter" && a.id === "newname") {
      e.preventDefault();
      act("create");
    } else if (e.key === "Enter" && a.id === "rename") {
      e.preventDefault();
      act("rename");
    }
    return;
  }
  const cur = entries[sel];
  if (e.key === "ArrowDown") { e.preventDefault(); select(sel + 1); }
  else if (e.key === "ArrowUp") { e.preventDefault(); select(sel - 1); }
  else if (e.key === "PageDown") { e.preventDefault(); select(sel + 10); }
  else if (e.key === "PageUp") { e.preventDefault(); select(sel - 10); }
  else if (e.key === "ArrowRight" && cur && ($q.value === "" || $q.selectionStart === $q.value.length)) {
    if (cur.t === "g") {
      e.preventDefault();
      if (!openKeys.has(cur.key)) toggle(cur.key, true);
      else select(sel + 1);
    }
  } else if (e.key === "ArrowLeft" && cur && ($q.value === "" || $q.selectionStart === 0)) {
    e.preventDefault();
    if (cur.t === "c") {
      const gi = entries.findIndex((x) => x.t === "g" && x.key === cur.key);
      if (gi >= 0) select(gi);
    } else if (openKeys.has(cur.key)) {
      toggle(cur.key, false);
    }
  } else if (e.key === "Enter") { e.preventDefault(); act(e.metaKey ? (e.shiftKey ? "path" : "paste") : "copy"); }
  else if (e.key === "Escape") {
    e.preventDefault();
    if ($q.value) { $q.value = ""; refresh({ keep: false }); } else invoke("hide");
  }
  else if (e.metaKey && e.key.toLowerCase() === "p") { e.preventDefault(); act("pin"); }
  else if (e.metaKey && e.key.toLowerCase() === "g") { e.preventDefault(); document.getElementById("grp")?.focus(); }
  else if (e.metaKey && e.key === "Backspace") { e.preventDefault(); act("delete"); }
  else if (document.activeElement !== $q && !e.metaKey && !e.ctrlKey && e.key.length === 1) $q.focus();
});

$list.addEventListener("click", (e) => {
  const li = e.target.closest("li[data-i]");
  if (!li) return;
  const star = e.target.closest("[data-star]");
  if (star) {
    e.stopPropagation();
    const ent = entries[Number(li.dataset.i)];
    if (ent?.t === "c") togglePin(ent.row);
    return;
  }
  const i = Number(li.dataset.i);
  const ent = entries[i];
  if (ent?.t === "g" && (e.target.closest(".chev") || sel === i)) toggle(ent.key);
  select(i);
});
$list.addEventListener("dblclick", (e) => {
  const li = e.target.closest("li.clip");
  if (li) act("copy");
});

// Drag clips onto group headers to move them.
$list.addEventListener("dragstart", (e) => {
  const li = e.target.closest("li.clip");
  if (!li) return;
  e.dataTransfer.setData("text/x-copypasta-clip", li.dataset.id);
  e.dataTransfer.effectAllowed = "move";
});
$list.addEventListener("dragover", (e) => {
  const li = e.target.closest("li.group");
  if (!li || !e.dataTransfer.types.includes("text/x-copypasta-clip")) return;
  e.preventDefault();
  $list.querySelectorAll(".dropping").forEach((x) => x.classList.remove("dropping"));
  li.classList.add("dropping");
});
$list.addEventListener("dragleave", (e) => e.target.closest?.("li.group")?.classList.remove("dropping"));
$list.addEventListener("drop", async (e) => {
  const li = e.target.closest("li.group");
  const id = Number(e.dataTransfer.getData("text/x-copypasta-clip"));
  $list.querySelectorAll(".dropping").forEach((x) => x.classList.remove("dropping"));
  if (!li || !id) return;
  e.preventDefault();
  const key = li.dataset.key;
  if (key === "pinned") {
    await call("pin", { id, pinned: true }).catch((err) => flash(String(err)));
    shownKey = null;
    await refresh();
    return;
  }
  await call("move", { id, group: key === "none" ? null : Number(key) }).catch((err) => flash(String(err)));
  await afterMove(id);
});

$detail.addEventListener("click", (e) => {
  const b = e.target.closest("button[data-act]");
  if (b) act(b.dataset.act);
});

let changeTimer = null;
T.event.listen("clips-changed", () => {
  clearTimeout(changeTimer);
  changeTimer = setTimeout(() => refresh({ keep: true }), 150);
});
T.event.listen("panel-shown", () => {
  $q.focus();
  $q.select();
  cursor = null;
  shownKey = null;
  refresh({ keep: false });
  renderStatus();
});

$q.focus();
refresh({ keep: false });
renderStatus();
