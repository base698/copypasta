// Lineage view. Related clips are cards, oldest first; each card lists where
// that clip was copied from and pasted into (repeats in one place collapsed),
// and a "↳" line says how it relates to an earlier clip. Shared helpers come
// from app.js globals.

function stepHtml(s) {
  const color = PLACE_COLOR[s.place] || PLACE_COLOR.app;
  const verb = s.kind === "copy" ? "Copied from" : "Pasted into";
  const ws = s.workspace ? `<span class="ws" title="herdr workspace">herdr ${esc(s.workspace)}</span>` : "";
  const n = s.count > 1 ? `<span class="times" title="${s.count} times, last ${esc(hhmm(s.last_ts))}">×${s.count}</span>` : "";
  return `<li class="step ${s.kind}">
    <span class="verb">${verb}</span>
    <span class="where"><i style="background:${color}"></i><b>${esc(s.title)}</b>${ws}${n}
      ${s.detail ? `<span class="det" title="${esc(s.detail)}">${esc(s.detail)}</span>` : ""}</span>
    <time>${esc(hhmm(s.ts))}</time>
  </li>`;
}

function renderDag(el, g) {
  if (!g.clips.some((c) => c.steps.length)) {
    el.innerHTML = `<div class="empty">No copy or paste events recorded for this clip yet.</div>`;
    return;
  }
  const rels = (id) => g.relations.filter((r) => r.child === id);
  let html = `<div class="flow">`;
  g.clips.forEach((c, i) => {
    const me = c.id === g.focus;
    if (i) html += `<div class="conn" aria-hidden="true"></div>`;
    html += `<article class="fclip ${me ? "focus" : "other"}" data-clip="${c.id}" ${me ? "" : 'title="Open this clip"'}>
      <header><span class="cid">#${c.id}</span><span class="cprev">${esc(c.preview)}</span>${me ? '<span class="me">this clip</span>' : ""}</header>
      ${rels(c.id).map((r) => `<div class="rel${r.weak ? " weak" : ""}">↳ ${esc(r.text)}${r.weak ? " <em>(probably)</em>" : ""}</div>`).join("")}
      <ol class="steps">${c.steps.map(stepHtml).join("")}</ol>
    </article>`;
  });
  html += `</div>`;
  el.innerHTML = html;
  el.querySelectorAll(".fclip.other").forEach((card) =>
    card.addEventListener("click", () => window.focusClip(Number(card.dataset.clip))),
  );
  el.querySelector(".fclip.focus")?.scrollIntoView({ block: "nearest" });
}
