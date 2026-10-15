// Lineage DAG renderer (inline SVG). Shared helpers come from app.js globals.
// ---- DAG layout: time flows down. Row = longest path from a root; each row
// is ordered by time and centered.
const NW = 236, NH = 66, GX = 18, GY = 46, PAD = 16;

function nodeTitle(n) {
  const h = n.ctx.herdr;
  if (h) {
    const who = h.agent || h.pane_label || "shell";
    const ws = h.workspace ? ` @ ${h.workspace}` : "";
    const tab = h.tab && h.tab !== h.workspace && !/^\d+$/.test(h.tab) ? ` · ${h.tab}` : "";
    return `${who}${ws}${tab}`;
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
    node.addEventListener("click", () => window.focusClip(Number(node.dataset.clip))),
  );
  el.querySelector(".node.focus")?.scrollIntoView({ block: "nearest", inline: "center" });
}


