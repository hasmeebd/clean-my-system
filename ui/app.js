'use strict';
/* System Care Suite frontend. Plain JS, no bundler. Talks to the Rust backend through Tauri invoke. */

if (!window.__TAURI__) {
  document.body.innerHTML = '<div style="padding:40px;font-family:Segoe UI">This page must run inside the System Care Suite app.</div>';
  throw new Error('No Tauri runtime');
}
const TAURI = window.__TAURI__;
const invoke = (c, a) => TAURI.core.invoke(c, a || {});
const listen = (e, cb) => TAURI.event.listen(e, cb);
const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const num = n => Number(n || 0).toLocaleString('en-US');
function fmt(b) {
  b = Number(b || 0);
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (b >= 1024 && i < 4) { b /= 1024; i++; }
  return (i < 2 ? b.toFixed(0) : b.toFixed(i >= 3 ? 2 : 1)) + ' ' + u[i];
}
const fdate = ts => ts ? new Date(ts * 1000).toISOString().slice(0, 10) : '–';
const fdt = ts => ts ? new Date(ts * 1000).toLocaleString() : '–';
const hash = s => { let h = 0; for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) >>> 0; return h; };

const S = {
  page: 'dashboard', admin: false, settings: null, sysDrive: 'C:', drive: null,
  ollama: { running: false, models: [] },
  hogs: null, hogSel: new Set(),
  folder: null, folderSel: new Set(), pendingPath: null, tmRects: [],
  large: null, largeSel: new Set(), largeRoot: 'C:\\', largeMin: 500,
  junk: null, junkSel: new Set(), junkRoots: null, junkIgnored: true, junkStack: new Set(), junkCollapsed: new Set(), junkRisk: 'all',
  dup: null, dupSel: new Set(), dupRoots: null, dupMin: 1, dupShown: 100, health: null, startup: null, startFilter: 'run', rcTab: 'bin', bin: [], binSel: new Set(), shPath: '',
  chat: [], aiReq: null, search: null, searchSel: new Set(),
};
const VIEWS = {};

// ------------------------------------------------------------------ UI helpers

function toast(msg, bad) {
  const t = document.createElement('div');
  t.className = 'toast' + (bad ? ' bad' : '');
  t.textContent = msg;
  $('#toasts').append(t);
  setTimeout(() => t.remove(), bad ? 8000 : 4500);
}
window.addEventListener('error', e => toast('UI error: ' + e.message, true));
window.addEventListener('unhandledrejection', e => toast(String(e.reason), true));

let busyN = 0;
async function busy(label, fn) {
  busyN++;
  $('#busy').classList.remove('hidden');
  $('#busyText').textContent = label + '…';
  try { return await fn(); }
  finally { busyN--; if (!busyN) $('#busy').classList.add('hidden'); }
}
listen('scan-progress', e => {
  if (!busyN) return;
  const p = e.payload;
  $('#busyText').textContent = `${p.label}… ${num(p.files)} items · ${fmt(p.bytes)}`;
});
$('#busyCancel').onclick = () => invoke('cancel_scan');

function modal({ title, body, buttons, onOpen }) {
  return new Promise(res => {
    const root = $('#modalRoot');
    root.innerHTML = `<div class="overlay"><div class="modal"><h3>${title}</h3><div class="mbody">${body}</div><div class="mfoot"></div></div></div>`;
    const foot = $('.mfoot', root);
    const close = value => {
      const data = {};
      $$('input,select,textarea', root).forEach(i => {
        if (!i.name) return;
        if (i.type === 'radio') { if (i.checked) data[i.name] = i.value; }
        else if (i.type === 'checkbox') data[i.name] = i.checked;
        else data[i.name] = i.value;
      });
      root.innerHTML = '';
      res({ value, data });
    };
    buttons.forEach(b => {
      const el = document.createElement('button');
      el.className = 'btn ' + (b.cls || '');
      el.textContent = b.label;
      el.onclick = () => (b.keep ? (b.onClick && b.onClick(el)) : close(b.value));
      foot.append(el);
    });
    if (onOpen) onOpen(root);
  });
}
const alertBox = (title, body) => modal({ title, body, buttons: [{ label: 'OK', value: true, cls: 'primary' }] });
const failList = f => f && f.length
  ? `<div style="margin-top:10px"><b>Could not remove ${f.length}:</b><ul class="plist">${f.slice(0, 8).map(x => `<li><code>${esc(x.path)}</code><div class="muted">${esc(x.reason)}</div></li>`).join('')}</ul></div>` : '';

// ------------------------------------------------------------------ chrome: nav, drive bar, badges

const PAGES = [
  ['dashboard', 'Dashboard', '◧'], ['health', 'System Health', '♥'], ['clean', 'Safe Clean', '✦'], ['explorer', 'Explorer', '▦'],
  ['large', 'Large Files', '⬢'], ['dupes', 'Duplicates', '⧉'], ['junk', 'Dev Junk', '⌬'], ['startup', 'Startup', '⏻'], ['optimize', 'Optimize', '⚡'], ['apps', 'Programs', '▤'], ['updates', 'Updates', '⇪'], ['procs', 'Processes', '▥'], ['browsers', 'Browser Privacy', '◍'], ['schedule', 'Schedule', '◷'], ['diag', 'Diagnostics', '✱'],
  ['fix', 'Troubleshoot', '✚'], ['recover', 'Recover Files', '↺'], ['ai', 'AI Assistant', '✺'],
  ['quarantine', 'Quarantine & History', '⟲'], ['settings', 'Settings', '⚙'],
];
function buildNav() {
  $('#nav').innerHTML = PAGES.map(([id, l, ic]) => `<button data-p="${id}"><span class="ic">${ic}</span>${l}</button>`).join('');
  $$('#nav button').forEach(b => b.onclick = () => go(b.dataset.p));
}
function go(p) {
  S.page = p;
  $$('#nav button').forEach(b => b.classList.toggle('on', b.dataset.p === p));
  Promise.resolve(VIEWS[p]()).catch(e => toast(String(e), true));
  $('#view').scrollTop = 0;
}
async function refreshDrive() {
  const d = await invoke('drive_info_cmd', { letter: S.sysDrive }).catch(() => null);
  if (!d) { $('#driveBar').innerHTML = '<span class="muted">Drive information unavailable</span>'; return; }
  S.drive = d;
  const pct = d.used / d.total * 100;
  const cls = pct > 90 ? 'bad' : pct > 75 ? 'warn' : '';
  $('#driveBar').innerHTML = `<div class="dtext"><b>${esc(d.letter)}</b> &nbsp;${fmt(d.total)}</div>
    <div class="dbar"><i class="${cls}" style="width:${pct.toFixed(1)}%"></i></div>
    <div class="dtext">Used <b>${fmt(d.used)}</b> (${pct.toFixed(1)}%) · Free <b>${fmt(d.free)}</b></div>`;
}
async function refreshBadges() {
  S.admin = await invoke('is_admin_cmd').catch(() => false);
  $('#adminBadge').innerHTML = S.admin
    ? '<span class="pill"><i class="dot ok"></i>Administrator</span>'
    : '<span class="pill"><i class="dot warn"></i>Standard user</span>';
  const o = await invoke('ollama_status').catch(() => ({ running: false, models: [] }));
  S.ollama = o;
  $('#ollamaBadge').innerHTML = `<span class="pill"><i class="dot ${o.running ? 'ok' : 'bad'}"></i>Ollama ${o.running ? o.models.length + ' models' : 'offline'}</span>`;
}
const adminBanner = () => S.admin ? '' :
  `<div class="banner warn"><span>Some Windows locations (Windows Temp, update cache, error reports) need Administrator rights.</span><span class="spacer"></span><button class="btn sm" id="btnAdmin">Restart as Administrator</button></div>`;
function bindAdmin() {
  const b = $('#btnAdmin');
  if (!b) return;
  b.onclick = async () => {
    const r = await modal({ title: 'Restart as Administrator?', body: 'The app closes and reopens through the Windows UAC prompt. If you decline the prompt, just start the app again.', buttons: [{ label: 'Cancel', value: false }, { label: 'Restart', value: true, cls: 'primary' }] });
    if (r.value) invoke('relaunch_admin').catch(e => toast(String(e), true));
  };
}

// ------------------------------------------------------------------ shared flows

async function explainPath(path) {
  if (!S.ollama.running) { toast('Ollama is not running. Start it and try again.', true); return; }
  try {
    const r = await busy('Asking ' + S.settings.model, () => invoke('ai_explain', { path, model: S.settings.model, lang: S.settings.lang }));
    await modal({
      title: 'AI explanation <span class="tag ai">AI suggestion · verify first</span>',
      body: `<div class="kv"><div>Path</div><div><code>${esc(path)}</code></div>
        <div>What it is</div><div>${esc(r.what)}</div>
        <div>Risk</div><div><span class="tag ${esc(r.risk)}">${esc(r.risk)}</span> ${r.system_note ? `<span class="muted">locked by safety rule: ${esc(r.system_note)}</span>` : ''}</div>
        <div>Reason</div><div>${esc(r.reason)}</div>
        <div>Advice</div><div>${esc(r.advice)}</div></div>
        <details style="margin-top:12px"><summary class="muted">Facts sent to the local model (nothing leaves this PC)</summary><pre style="white-space:pre-wrap">${esc(r.facts)}</pre></details>`,
      buttons: [{ label: 'Close', value: true, cls: 'primary' }],
    });
  } catch (e) { toast(String(e), true); }
}

async function deleteFlow(items, kind, opts) {
  opts = opts || {};
  if (!items.length) return false;
  const defPerm = opts.defaultMode === 'permanent';
  const total = items.reduce((s, i) => s + (i.size || 0), 0);
  const days = S.settings.quarantine_days;
  const list = items.slice(0, 8).map(i => `<li><code>${esc(i.path)}</code> <span class="muted">${fmt(i.size)}</span></li>`).join('')
    + (items.length > 8 ? `<li class="muted">…and ${items.length - 8} more</li>` : '');
  const r = await modal({
    title: `Remove ${items.length} item(s) · ${fmt(total)}`,
    body: `${opts.extraHtml || ''}<ul class="plist">${list}</ul>
      <label class="radio"><input type="radio" name="mode" value="quarantine" ${defPerm ? '' : 'checked'}> <b>Move to Quarantine</b> <span class="muted">can be restored; disk space returns when purged${days ? ` (auto-purge after ${days} days)` : ''}</span></label>
      <label class="radio"><input type="radio" name="mode" value="permanent" ${defPerm ? 'checked' : ''}> <b>Delete permanently</b> <span class="muted">frees space now, cannot be undone${defPerm ? ' (recommended for rebuildable folders: moving thousands of files to Quarantine is slow)' : ''}</span></label>
      <label class="check ${defPerm ? '' : 'hidden'}" id="ackRow"><input type="checkbox" name="ack"> I understand this cannot be undone</label>`,
    onOpen: root => $$('input[name=mode]', root).forEach(i => i.onchange = () =>
      $('#ackRow', root).classList.toggle('hidden', $('input[name=mode]:checked', root).value !== 'permanent')),
    buttons: [{ label: 'Cancel', value: false }, { label: 'Remove', value: true, cls: 'danger' }],
  });
  if (!r.value) return false;
  if (r.data.mode === 'permanent' && !r.data.ack) { toast('Tick the confirmation to delete permanently.', true); return false; }
  const rep = await busy('Removing', () => (opts.remove ? opts.remove(r.data.mode) : invoke('delete_paths', { paths: items.map(i => i.path), mode: r.data.mode, kind: kind || null })))
    .catch(e => { toast(String(e), true); return null; });
  if (!rep) return false;
  await alertBox(rep.ok ? 'Done' : 'Nothing was removed',
    `<div><b>${rep.ok}</b> item(s) ${rep.mode === 'permanent' ? 'deleted' : 'moved to quarantine'} · ${fmt(rep.bytes)}</div>${failList(rep.failed)}`);
  await refreshDrive();
  return rep.ok > 0;
}

function selBar(el, sel, getSize, onRemove, extra) {
  if (!sel.size) { el.innerHTML = ''; return; }
  let t = 0; sel.forEach(p => t += getSize(p) || 0);
  el.innerHTML = `<div class="sel-bar"><b>${sel.size} selected</b><span class="muted">${fmt(t)}</span><span class="spacer"></span>${extra || ''}<button class="btn danger" id="selRemove">Remove…</button></div>`;
  $('#selRemove', el).onclick = onRemove;
}

// ------------------------------------------------------------------ Dashboard

function chartSvg(snaps) {
  if (snaps.length < 2) return '<div class="muted">Free-space history appears after a few launches or cleanups.</div>';
  const W = 700, H = 150, P = 8;
  const xs = snaps.map(s => s.ts), ys = snaps.map(s => s.free / 1e9);
  const x0 = Math.min(...xs), x1 = Math.max(...xs), y0 = Math.min(...ys), y1 = Math.max(...ys);
  const sx = t => P + (x1 === x0 ? 0 : (t - x0) / (x1 - x0)) * (W - 2 * P);
  const sy = v => H - P - (y1 === y0 ? .5 : (v - y0) / (y1 - y0)) * (H - 2 * P);
  const pts = snaps.map(s => `${sx(s.ts).toFixed(1)},${sy(s.free / 1e9).toFixed(1)}`).join(' ');
  return `<svg class="chart" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none"><polyline fill="none" stroke="#2dd4bf" stroke-width="2.5" points="${pts}"/></svg>
    <div class="row muted"><span>low ${y0.toFixed(0)} GB</span><span class="spacer"></span><span>high ${y1.toFixed(0)} GB free</span></div>`;
}

VIEWS.dashboard = async () => {
  const v = $('#view');
  v.innerHTML = '<div class="empty">Loading…</div>';
  await refreshDrive(); await refreshBadges();
  const [snaps, q, hist, drives] = await Promise.all([invoke('get_snapshots'), invoke('list_quarantine'), invoke('get_history'), invoke('list_drives')]);
  const qBytes = q.reduce((s, i) => s + i.size, 0);
  const rec = hist.filter(h => ['safe-clean', 'delete', 'purge', 'tool:hibernate_off', 'tool:dism_cleanup'].includes(h.action)).reduce((s, h) => s + h.bytes, 0);
  const d = S.drive;
  const delta = snaps.length ? d.free - snaps[0].free : 0;
  v.innerHTML = `<h2>Dashboard</h2><div class="sub">Where your disk stands and what to do next.</div>${adminBanner()}
    <div class="grid g4">
      <div class="card"><div class="lbl">Free on ${esc(d.letter)}</div><div class="big">${fmt(d.free)}</div><div class="muted">${snaps.length > 1 ? (delta >= 0 ? '+' : '−') + fmt(Math.abs(delta)) + ' since first launch' : 'tracking started'}</div></div>
      <div class="card"><div class="lbl">Used</div><div class="big">${(d.used / d.total * 100).toFixed(1)}%</div><div class="muted">${fmt(d.used)} of ${fmt(d.total)}</div></div>
      <div class="card"><div class="lbl">In quarantine</div><div class="big">${fmt(qBytes)}</div><div class="muted">${q.length} item(s), restorable</div></div>
      <div class="card"><div class="lbl">Recovered by this app</div><div class="big">${fmt(rec)}</div><div class="muted">${hist.length} logged action(s)</div></div>
    </div>
    <div class="grid g2" style="margin-top:14px">
      <div class="card"><h3>Free space over time</h3>${chartSvg(snaps)}</div>
      <div class="card"><h3>Start here</h3>
        <div class="grid" style="gap:8px">
          <button class="btn primary" id="dbClean">1 · Scan known junk (Safe Clean)</button>
          <button class="btn" id="dbEx">2 · Analyze ${esc(d.letter)}\\ folder by folder</button>
          <button class="btn" id="dbLarge">3 · Find the biggest files</button>
          <button class="btn" id="dbAi">4 · Ask the AI assistant for a plan</button>
        </div>
        <h3 style="margin-top:16px">Drives</h3>
        ${drives.map(x => `<div class="row" style="margin:6px 0"><b style="width:34px">${esc(x.letter)}</b><div class="dbar"><i style="width:${(x.used / x.total * 100).toFixed(1)}%"></i></div><span class="muted" style="width:150px;text-align:right">${fmt(x.free)} free</span></div>`).join('')}
      </div>
    </div>`;
  bindAdmin();
  v.insertAdjacentHTML('beforeend', '<div class="card" id="dbHealth" style="margin-top:14px"><div class="muted">Checking system health…</div></div>');
  invoke('health_check').then(r => {
    const el = $('#dbHealth'); if (!el || S.page !== 'dashboard') return;
    const issues = r.checks.filter(c => c.status === 'bad' || c.status === 'warn');
    el.innerHTML = `<div class="row" style="gap:22px">${scoreRing(r.score, r.grade)}<div style="flex:1;min-width:240px"><h3>System health</h3>
      ${issues.length ? issues.slice(0, 4).map(c => `<div style="margin:4px 0"><i class="dotc" style="background:${STATUS_COL[c.status]}"></i> <b>${esc(c.title)}</b> <span class="muted">${esc(c.detail)}</span></div>`).join('') : '<div class="muted">No problems found.</div>'}</div>
      <button class="btn primary" id="dbHealthGo">Open System Health</button></div>`;
    $('#dbHealthGo').onclick = () => go('health');
  }).catch(() => { const el = $('#dbHealth'); if (el) el.remove(); });
  $('#dbClean').onclick = () => { go('clean'); setTimeout(scanHogs, 50); };
  $('#dbEx').onclick = () => { S.pendingPath = S.drive.letter + '\\'; go('explorer'); };
  $('#dbLarge').onclick = () => go('large');
  $('#dbAi').onclick = () => go('ai');
};

// ------------------------------------------------------------------ Safe Clean

const TOOLS = [
  { id: 'hibernate_off', name: 'Turn off Hibernate', desc: 'Deletes hiberfil.sys (about the size of your RAM). Disables Hibernate and Fast Startup. Sleep and Shutdown still work.', admin: true },
  { id: 'dism_cleanup', name: 'Component Store cleanup (DISM)', desc: 'Official cleanup of superseded Windows Update components. Takes 5–15 minutes.', admin: true },
  { id: 'storage_sense', name: 'Remove Windows.old', desc: 'Opens Windows Storage settings: Temporary files, then Previous Windows installation(s).', admin: false },
  { id: 'hibernate_on', name: 'Turn Hibernate back on', desc: 'Restores Hibernate and Fast Startup.', admin: true },
];

function hogGroups() {
  const g = {};
  (S.hogs || []).forEach(h => {
    g[h.group] = g[h.group] || { name: h.group, items: [], size: 0 };
    g[h.group].items.push(h); g[h.group].size += h.size;
  });
  return Object.values(g).sort((a, b) => b.size - a.size);
}
function updateHogBar() {
  const sel = (S.hogs || []).filter(h => S.hogSel.has(h.id));
  const t = sel.reduce((s, h) => s + h.size, 0);
  $('#hogTotal').textContent = sel.length ? `${sel.length} selected · ${fmt(t)}` : '';
  $('#hogClean').disabled = !sel.length;
}
function renderHogs() {
  const box = $('#hogTable');
  if (!box || !S.hogs) return;
  const rows = hogGroups().map(g => `<tr class="group"><td colspan="5">${esc(g.name)} <span class="muted">· ${fmt(g.size)}</span></td></tr>` +
    g.items.map(h => {
      const locked = h.admin && h.auto && !S.admin;
      const can = h.auto && !locked && h.size > 0;
      const chk = h.auto ? `<input type="checkbox" data-id="${esc(h.id)}" ${S.hogSel.has(h.id) ? 'checked' : ''} ${can ? '' : 'disabled'}>` : '';
      const act = !h.auto && h.paths[0] ? `<button class="btn sm" data-open="${esc(h.paths[0])}">Open</button>` : '';
      return `<tr><td style="width:30px">${chk}</td><td><b>${esc(h.name)}</b>${locked ? ' 🔒' : ''}<div class="muted">${esc(h.tip)}</div></td>
        <td><span class="tag ${esc(h.risk)}">${esc(h.risk)}</span></td><td class="num">${fmt(h.size)}</td><td class="num">${act}</td></tr>`;
    }).join('')).join('');
  box.innerHTML = `<table><thead><tr><th></th><th>Location</th><th>Risk</th><th class="num">Size</th><th></th></tr></thead><tbody>${rows}</tbody></table>`;
  $$('input[data-id]', box).forEach(i => i.onchange = () => { i.checked ? S.hogSel.add(i.dataset.id) : S.hogSel.delete(i.dataset.id); updateHogBar(); });
  $$('button[data-open]', box).forEach(b => b.onclick = () => { S.pendingPath = b.dataset.open; go('explorer'); });
  $('#hogSafe').disabled = false; $('#hogNone').disabled = false;
  updateHogBar();
}
async function scanHogs() {
  if (S.page !== 'clean') return;
  const r = await busy('Scanning known junk', () => invoke('list_hogs')).catch(e => { toast(String(e), true); return null; });
  if (!r) return;
  S.hogs = r; S.hogSel = new Set();
  renderHogs();
}
async function cleanSelected() {
  const sel = S.hogs.filter(h => S.hogSel.has(h.id));
  const total = sel.reduce((s, h) => s + h.size, 0);
  const review = sel.some(h => h.risk === 'review');
  const r = await modal({
    title: `Clean ${sel.length} location(s) · ${fmt(total)}`,
    body: `<ul class="plist">${sel.map(h => `<li>${esc(h.name)} <span class="muted">${fmt(h.size)}</span></li>`).join('')}</ul>
      <div class="muted">Contents are deleted permanently. These locations rebuild themselves or re-download when needed.${review ? '<br><b>Includes items marked review:</b> the next build or download will be slower.' : ''}</div>`,
    buttons: [{ label: 'Cancel', value: false }, { label: 'Clean now', value: true, cls: 'primary' }],
  });
  if (!r.value) return;
  const rep = await busy('Cleaning', () => invoke('clean_hogs', { ids: sel.map(h => h.id) })).catch(e => { toast(String(e), true); return null; });
  if (!rep) return;
  await alertBox('Clean complete',
    `<div style="font-size:22px;font-weight:600">${fmt(rep.freed_on_disk)} <span class="muted" style="font-size:13px;font-weight:400">free space gained (measured on disk)</span></div>
     <table style="margin-top:10px"><tbody>${rep.items.map(i => `<tr><td>${esc(i.name)}</td><td class="num">${fmt(i.freed)}</td><td class="muted">${esc(i.note)}</td></tr>`).join('')}</tbody></table>`);
  await refreshDrive();
  await scanHogs();
}
async function runTool(t) {
  if (t.admin && !S.admin) { toast('This tool needs Administrator rights. Use “Restart as Administrator”.', true); return; }
  const r = await modal({ title: t.name, body: esc(t.desc), buttons: [{ label: 'Cancel', value: false }, { label: 'Run', value: true, cls: 'primary' }] });
  if (!r.value) return;
  const out = await busy(t.name, () => invoke('run_tool', { id: t.id })).catch(e => { toast(String(e), true); return null; });
  if (!out) return;
  await alertBox(t.name, `<div>Freed on disk: <b>${fmt(out.freed)}</b></div><pre style="white-space:pre-wrap;margin-top:8px">${esc(out.output)}</pre>`);
  await refreshDrive();
}
VIEWS.clean = () => {
  $('#view').innerHTML = `<h2>Safe Clean</h2><div class="sub">Only an allowlist of known cache and temp locations is ever touched. Browser cookies, history and passwords are not included.</div>${adminBanner()}
    <div class="row" style="margin-bottom:12px">
      <button class="btn primary" id="hogScan">Scan</button>
      <button class="btn" id="hogSafe" disabled>Select all safe</button>
      <button class="btn" id="hogNone" disabled>Clear selection</button>
      <span class="spacer"></span><span id="hogTotal" class="muted"></span>
      <button class="btn primary" id="hogClean" disabled>Clean selected</button>
    </div>
    <div id="hogTable">${S.hogs ? '' : '<div class="empty">Press <b>Scan</b> to measure every known junk location.</div>'}</div>
    <h3 style="margin-top:24px">System tools</h3>
    <div class="grid g2" id="tools">${TOOLS.map(t => `<div class="card"><b>${esc(t.name)}</b>${t.admin ? ' <span class="tag info">admin</span>' : ''}<div class="muted" style="margin:6px 0 10px">${esc(t.desc)}</div><button class="btn sm" data-tool="${t.id}">Run…</button></div>`).join('')}</div>`;
  bindAdmin();
  $('#hogScan').onclick = scanHogs;
  $('#hogSafe').onclick = () => {
    S.hogs.forEach(h => { if (h.auto && h.risk === 'safe' && h.size > 0 && !(h.admin && !S.admin)) S.hogSel.add(h.id); });
    renderHogs();
  };
  $('#hogNone').onclick = () => { S.hogSel.clear(); renderHogs(); };
  $('#hogClean').onclick = cleanSelected;
  $$('[data-tool]').forEach(b => b.onclick = () => runTool(TOOLS.find(t => t.id === b.dataset.tool)));
  if (S.hogs) renderHogs();
};

// ------------------------------------------------------------------ Explorer

function squarify(items, W, H) {
  const total = items.reduce((s, i) => s + i.v, 0) || 1;
  const nodes = items.map(i => ({ ...i, a: i.v / total * W * H }));
  const rects = [];
  let x = 0, y = 0, w = W, h = H, row = [];
  const worst = (r, side) => {
    const s = r.reduce((t, n) => t + n.a, 0);
    const mx = Math.max(...r.map(n => n.a)), mn = Math.min(...r.map(n => n.a));
    return Math.max(side * side * mx / (s * s), s * s / (side * side * mn));
  };
  const layout = r => {
    const s = r.reduce((t, n) => t + n.a, 0);
    if (w >= h) {
      const rw = s / h; let yy = y;
      r.forEach(n => { const rh = n.a / rw; rects.push({ ...n, x, y: yy, w: rw, h: rh }); yy += rh; });
      x += rw; w -= rw;
    } else {
      const rh = s / w; let xx = x;
      r.forEach(n => { const rw = n.a / rh; rects.push({ ...n, x: xx, y, w: rw, h: rh }); xx += rw; });
      y += rh; h -= rh;
    }
  };
  for (const n of nodes) {
    const side = Math.min(w, h);
    if (row.length && worst([...row, n], side) > worst(row, side)) { layout(row); row = []; }
    row.push(n);
  }
  if (row.length) layout(row);
  return rects;
}
function drawTreemap(entries) {
  const cv = $('#tm');
  if (!cv) return;
  const dpr = window.devicePixelRatio || 1;
  const W = cv.clientWidth, H = cv.clientHeight;
  cv.width = W * dpr; cv.height = H * dpr;
  const g = cv.getContext('2d');
  g.scale(dpr, dpr); g.clearRect(0, 0, W, H);
  const items = entries.filter(e => e.size > 0).slice(0, 70).map(e => ({ e, v: e.size }));
  if (!items.length) return;
  const rects = squarify(items, W, H);
  S.tmRects = rects;
  rects.forEach(r => {
    const e = r.e;
    g.fillStyle = e.is_dir ? `hsl(${hash(e.name) % 360} 50% 34%)` : 'hsl(212 22% 30%)';
    g.fillRect(r.x + 1, r.y + 1, Math.max(0, r.w - 2), Math.max(0, r.h - 2));
    if (r.w > 74 && r.h > 36) {
      g.fillStyle = '#fff'; g.font = '12px Segoe UI';
      const max = Math.floor((r.w - 14) / 6.3);
      g.fillText(e.name.length > max ? e.name.slice(0, max - 1) + '…' : e.name, r.x + 7, r.y + 18);
      g.fillStyle = 'rgba(255,255,255,.7)';
      g.fillText(fmt(e.size), r.x + 7, r.y + 34);
    }
  });
  const hit = ev => {
    const b = cv.getBoundingClientRect();
    const px = ev.clientX - b.left, py = ev.clientY - b.top;
    return S.tmRects.find(r => px >= r.x && px <= r.x + r.w && py >= r.y && py <= r.y + r.h);
  };
  cv.onmousemove = ev => { const r = hit(ev); cv.title = r ? `${r.e.name} · ${fmt(r.e.size)}` : ''; };
  cv.onclick = ev => { const r = hit(ev); if (r && r.e.is_dir && !r.e.link) openFolder(r.e.path); };
}
async function openFolder(path) {
  const fv = await busy('Analyzing', () => invoke('analyze_folder', { path })).catch(e => { toast(String(e), true); return null; });
  if (!fv) return;
  S.folder = fv; S.folderSel = new Set();
  const inp = $('#exPath'); if (inp) inp.value = fv.path;
  renderFolder();
}
function crumbsHtml(p) {
  const parts = p.split('\\').filter(Boolean);
  let acc = '';
  return parts.map((x, i) => {
    acc = i === 0 ? x + '\\' : (acc.endsWith('\\') ? acc : acc + '\\') + x;
    return `<a data-go="${esc(acc)}">${esc(x)}</a>`;
  }).join(' <span class="muted">›</span> ');
}
function renderFolder() {
  const fv = S.folder, box = $('#exBody');
  if (!fv || !box) return;
  const total = fv.total || 1, shown = fv.entries.slice(0, 300);
  const sparse = fv.logical > fv.total * 1.05 && fv.logical - fv.total > 1e9;
  box.innerHTML = `<div class="crumbs">${crumbsHtml(fv.path)}</div>
    <div class="row" style="margin-bottom:10px">${fv.parent ? '<button class="btn sm" id="exUp">⬆ Up</button>' : ''}
      <b>${fmt(fv.total)}</b><span class="muted">on disk in this folder${sparse ? ` · <span class="sparse">logical ${fmt(fv.logical)} (sparse or compressed files inflate it)</span>` : ''}</span>
      <span class="spacer"></span><button class="btn sm" id="exReveal">Show in Windows Explorer</button></div>
    <div class="split"><div><table><thead><tr><th></th><th>Name</th><th>Share</th><th class="num">Size</th><th class="num">Modified</th><th></th></tr></thead><tbody>
      ${shown.map((e, i) => `<tr><td style="width:30px"><input type="checkbox" data-sel="${i}" ${S.folderSel.has(e.path) ? 'checked' : ''} ${e.link ? 'disabled' : ''}></td>
        <td><span class="path">${e.is_dir && !e.link ? `<a class="lnk" data-open="${i}">📁 ${esc(e.name)}</a>` : (e.link ? '🔗 ' : '📄 ') + esc(e.name)}</span></td>
        <td style="width:110px"><div class="bar"><i style="width:${Math.min(100, e.size / total * 100).toFixed(1)}%"></i></div></td>
        <td class="num">${fmt(e.size)}</td><td class="num muted">${fdate(e.modified)}</td>
        <td class="num"><button class="btn sm" data-ai="${i}">AI</button></td></tr>`).join('')}
      </tbody></table>${fv.entries.length > 300 ? `<div class="muted" style="padding:8px">Showing the largest 300 of ${fv.entries.length} entries.</div>` : ''}</div>
      <div class="card"><h3>Treemap</h3><canvas id="tm"></canvas><div class="muted" style="margin-top:6px">Click a folder block to open it. Hover for details.</div></div></div>
    <div id="exSel"></div>`;
  $$('[data-go]', box).forEach(a => a.onclick = () => openFolder(a.dataset.go));
  const up = $('#exUp'); if (up) up.onclick = () => openFolder(fv.parent);
  $('#exReveal').onclick = () => invoke('reveal_in_explorer', { path: fv.path });
  $$('[data-open]', box).forEach(a => a.onclick = () => openFolder(shown[+a.dataset.open].path));
  $$('[data-ai]', box).forEach(b => b.onclick = () => explainPath(shown[+b.dataset.ai].path));
  const upd = () => selBar($('#exSel'), S.folderSel,
    p => (fv.entries.find(e => e.path === p) || {}).size,
    async () => {
      const items = [...S.folderSel].map(p => fv.entries.find(e => e.path === p)).filter(Boolean).map(e => ({ path: e.path, size: e.size }));
      if (await deleteFlow(items)) openFolder(fv.path);
    });
  $$('input[data-sel]', box).forEach(i => i.onchange = () => {
    const p = shown[+i.dataset.sel].path;
    i.checked ? S.folderSel.add(p) : S.folderSel.delete(p); upd();
  });
  upd();
  requestAnimationFrame(() => drawTreemap(fv.entries));
}
VIEWS.explorer = async () => {
  const quick = await invoke('quick_paths');
  const drives = await invoke('list_drives');
  $('#view').innerHTML = `<h2>Explorer</h2><div class="sub">Folder sizes by real space on disk. Sparse files and cloud-only placeholders are measured correctly. Hard-linked files (Windows components, uv/pip caches) can be counted once per link, so a total may slightly exceed the drive's used space.</div>
    <div class="row"><input type="text" id="exPath" style="flex:1;min-width:280px" placeholder="C:\\  or any folder path" value="${esc(S.folder ? S.folder.path : S.drive.letter + '\\')}">
      <button class="btn primary" id="exGo">Analyze</button></div>
    <div class="chips" style="margin:10px 0">${drives.map(d => `<span class="chip" data-p="${esc(d.letter + '\\')}">💽 ${esc(d.letter)}</span>`).join('')}${quick.map(q => `<span class="chip" data-p="${esc(q.path)}">${esc(q.label)}</span>`).join('')}</div>
    <div id="exBody">${S.folder ? '' : '<div class="empty">Choose a drive or folder, then press <b>Analyze</b>. A full C:\\ scan takes under a minute on most SSDs.</div>'}</div>`;
  $('#exGo').onclick = () => openFolder($('#exPath').value);
  $('#exPath').onkeydown = e => { if (e.key === 'Enter') openFolder($('#exPath').value); };
  $$('.chip[data-p]').forEach(c => c.onclick = () => openFolder(c.dataset.p));
  if (S.pendingPath) { const p = S.pendingPath; S.pendingPath = null; openFolder(p); }
  else if (S.folder) renderFolder();
};

// ------------------------------------------------------------------ Large Files

function fileTable(rows, selSet, idPrefix) {
  return `<table><thead><tr><th></th><th>File</th><th class="num">On disk</th><th class="num">Logical</th><th class="num">Modified</th><th></th></tr></thead><tbody>
    ${rows.map((f, i) => {
      const sp = f.logical > f.size * 2 && f.logical > 1e9;
      return `<tr><td style="width:30px"><input type="checkbox" data-${idPrefix}="${i}" ${selSet.has(f.path) ? 'checked' : ''}></td>
        <td><b>${esc(f.name)}</b><div class="muted path"><code>${esc(f.path)}</code></div></td>
        <td class="num">${fmt(f.size)}</td><td class="num ${sp ? 'sparse' : 'muted'}">${fmt(f.logical)}${sp ? '<div>sparse</div>' : ''}</td>
        <td class="num muted">${fdate(f.modified)}</td>
        <td class="num"><button class="btn sm" data-${idPrefix}-show="${i}">Show</button> <button class="btn sm" data-${idPrefix}-ai="${i}">AI</button></td></tr>`;
    }).join('')}</tbody></table>`;
}
function bindFileTable(box, rows, selSet, idPrefix, barEl, after) {
  const upd = () => selBar(barEl, selSet, p => (rows.find(r => r.path === p) || {}).size,
    async () => {
      const items = [...selSet].map(p => rows.find(r => r.path === p)).filter(Boolean).map(r => ({ path: r.path, size: r.size }));
      if (await deleteFlow(items)) after(items.map(i => i.path));
    });
  $$(`input[data-${idPrefix}]`, box).forEach(i => i.onchange = () => {
    const p = rows[+i.getAttribute('data-' + idPrefix)].path;
    i.checked ? selSet.add(p) : selSet.delete(p); upd();
  });
  $$(`[data-${idPrefix}-show]`, box).forEach(b => b.onclick = () => invoke('reveal_in_explorer', { path: rows[+b.getAttribute('data-' + idPrefix + '-show')].path }));
  $$(`[data-${idPrefix}-ai]`, box).forEach(b => b.onclick = () => explainPath(rows[+b.getAttribute('data-' + idPrefix + '-ai')].path));
  upd();
}
function renderLarge() {
  const box = $('#lgBody');
  if (!box || !S.large) return;
  if (!S.large.length) { box.innerHTML = '<div class="empty">No files above that size.</div>'; return; }
  const tot = S.large.reduce((s, f) => s + f.size, 0);
  box.innerHTML = `<div class="muted" style="margin-bottom:8px">${S.large.length} files · ${fmt(tot)} on disk</div>${fileTable(S.large, S.largeSel, 'lg')}<div id="lgSel"></div>`;
  bindFileTable(box, S.large, S.largeSel, 'lg', $('#lgSel'), gone => {
    S.large = S.large.filter(f => !gone.includes(f.path)); S.largeSel = new Set(); renderLarge();
  });
}
VIEWS.large = () => {
  $('#view').innerHTML = `<h2>Large Files</h2><div class="sub">Largest files by real size on disk. A big gap between “on disk” and “logical” means a sparse file, such as an emulator disk image.</div>
    <div class="row"><input type="text" id="lgRoot" style="flex:1;min-width:260px" value="${esc(S.largeRoot)}">
      <label class="muted">Min size (MB) <input type="number" id="lgMin" value="${S.largeMin}" style="width:90px"></label>
      <button class="btn primary" id="lgGo">Scan</button></div>
    <div id="lgBody" style="margin-top:14px">${S.large ? '' : '<div class="empty">Press <b>Scan</b> to list the biggest files.</div>'}</div>`;
  $('#lgGo').onclick = async () => {
    S.largeRoot = $('#lgRoot').value; S.largeMin = +$('#lgMin').value || 500;
    const r = await busy('Searching large files', () => invoke('find_large_files', { root: S.largeRoot, minMb: S.largeMin, limit: 200 })).catch(e => { toast(String(e), true); return null; });
    if (!r) return;
    S.large = r; S.largeSel = new Set(); renderLarge();
  };
  renderLarge();
};

// ------------------------------------------------------------------ Dev Junk

const daysAgo = ts => ts ? Math.floor((Date.now() / 1000 - ts) / 86400) : null;
const COST_TAG = { low: 'safe', medium: 'review', high: 'keep' };
const COST_LBL = { low: 'quick rebuild', medium: 'moderate rebuild', high: 'slow rebuild' };

function jkVisible() {
  const st = S.junkStack, rk = S.junkRisk;
  return S.junk.entries.filter(e => (!st.size || st.has(e.stack)) && (rk === 'all' || e.risk === rk));
}
const jkStale = p => { const d = daysAgo(p && p.last_active); return d !== null && d >= (S.settings.junk_stale_days || 30); };
function gitBadge(g) {
  if (!g || !g.repo) return '<span class="tag info">no git</span>';
  if (!g.known) return '<span class="tag info" title="git could not be read for this repository">git ?</span>';
  return g.dirty ? `<span class="tag review" title="Uncommitted changes. Your source files are never touched by this tool.">${g.dirty} uncommitted</span>` : '<span class="tag safe">git clean</span>';
}
function jkGroups(vis) {
  const visSet = new Set(vis.map(e => e.path));
  const g = new Map();
  for (const e of S.junk.entries) if (visSet.has(e.path)) { if (!g.has(e.project)) g.set(e.project, []); g.get(e.project).push(e); }
  return [...g.entries()].map(([path, items]) => ({
    path, items, p: S.junk.projects[path] || { name: path.split('\\').pop(), last_active: 0, git: {} },
    total: items.reduce((s, i) => s + i.size, 0),
  })).sort((a, b) => b.total - a.total);
}
const sumSize = a => a.reduce((s, i) => s + i.size, 0);

function jkRemoveFlow(items) {
  const byProj = new Map();
  items.forEach(i => byProj.set(i.project, S.junk.projects[i.project]));
  const dirty = [...byProj.values()].filter(p => p && p.git && p.git.dirty > 0);
  const slow = items.filter(i => i.cost === 'high');
  const cmds = [...new Set(items.map(i => i.rebuild))].slice(0, 6);
  const allSafe = items.every(i => i.risk === 'safe');
  const extra = `<div class="muted" style="margin-bottom:8px">${byProj.size} project(s). To restore: ${cmds.map(c => `<code>${esc(c)}</code>`).join(', ')}</div>
    ${slow.length ? `<div class="banner warn" style="padding:6px 10px"><span>${slow.length} item(s) are slow to rebuild (${[...new Set(slow.map(i => i.kind))].map(esc).join(', ')}).</span></div>` : ''}
    ${dirty.length ? `<div class="banner info" style="padding:6px 10px"><span>${dirty.length} selected project(s) have uncommitted changes: ${dirty.slice(0, 4).map(p => `<b>${esc(p.name)}</b>`).join(', ')}. Only generated folders are removed, never your source files.</span></div>` : ''}
    ${allSafe ? '' : '<div class="banner warn" style="padding:6px 10px"><span>Some items are marked <b>review</b>: they are probably regenerable, but look inside first. Quarantine is pre-selected.</span></div>'}`;
  return deleteFlow(items.map(i => ({ path: i.path, size: i.size, project: i.project })), 'junk', { defaultMode: allSafe ? 'permanent' : 'quarantine', extraHtml: extra });
}

function renderJunk() {
  const box = $('#jkBody');
  if (!box || !S.junk) return;
  const r = S.junk;
  const note = r.small_count ? `<div class="muted" style="margin-top:10px">${num(r.small_count)} tiny item(s) (${fmt(r.small_bytes)} total) were left out.</div>` : '';
  if (!r.entries.length) { box.innerHTML = '<div class="empty">Nothing regenerable found in these folders.</div>' + note; return; }
  const vis = jkVisible();
  const groups = jkGroups(vis);
  const staleDays = S.settings.junk_stale_days || 30;
  const staleItems = groups.filter(g => jkStale(g.p)).flatMap(g => g.items).filter(e => e.risk === 'safe');
  const dirtyN = groups.filter(g => g.p.git && g.p.git.dirty > 0).length;
  const stk = new Map();
  r.entries.forEach(e => { const o = stk.get(e.stack) || { n: 0, b: 0 }; o.n++; o.b += e.size; stk.set(e.stack, o); });

  box.innerHTML = `
    ${r.git_available ? '' : '<div class="banner warn"><span>Git was not found on this PC. Project age uses file dates only and git-ignored folders cannot be detected.</span></div>'}
    <div class="grid g4">
      <div class="card"><div class="lbl">Reclaimable (shown)</div><div class="big">${fmt(sumSize(vis))}</div><div class="muted">${vis.length} folder(s) in ${groups.length} project(s)</div></div>
      <div class="card"><div class="lbl">Marked safe</div><div class="big">${fmt(sumSize(vis.filter(e => e.risk === 'safe')))}</div><div class="muted">verified build output and caches</div></div>
      <div class="card"><div class="lbl">Inactive ${staleDays}+ days</div><div class="big">${fmt(sumSize(staleItems))}</div><div class="muted">${new Set(staleItems.map(e => e.project)).size} project(s), safe items only</div></div>
      <div class="card"><div class="lbl">Uncommitted work</div><div class="big">${dirtyN}</div><div class="muted">project(s) with git changes (never touched)</div></div>
    </div>
    <div class="chips" style="margin:14px 0 8px"><span class="chip ${S.junkStack.size ? '' : 'on'}" data-stk="">All stacks</span>
      ${[...stk.entries()].sort((a, b) => b[1].b - a[1].b).map(([k, v]) => `<span class="chip ${S.junkStack.has(k) ? 'on' : ''}" data-stk="${esc(k)}">${esc(k)} · ${fmt(v.b)}</span>`).join('')}</div>
    <div class="row" style="margin-bottom:10px">
      <label class="muted">Show <select id="jkRisk"><option value="all" ${S.junkRisk === 'all' ? 'selected' : ''}>everything</option><option value="safe" ${S.junkRisk === 'safe' ? 'selected' : ''}>safe only</option><option value="review" ${S.junkRisk === 'review' ? 'selected' : ''}>review only</option></select></label>
      <label class="muted">Inactive for <input type="number" id="jkDays" min="1" value="${staleDays}" style="width:70px"> days</label>
      <span class="spacer"></span>
      <button class="btn sm" id="jkSafe">Select all safe</button>
      <button class="btn sm" id="jkStale">Select safe items in inactive projects</button>
      <button class="btn sm" id="jkClear">Clear</button></div>
    <table><thead><tr><th></th><th>Project / folder</th><th>What to run afterwards</th><th class="num">Size</th></tr></thead><tbody>
    ${groups.map(g => {
      const d = daysAgo(g.p.last_active), stale = jkStale(g.p), col = S.junkCollapsed.has(g.path);
      const allSel = g.items.every(e => S.junkSel.has(e.path));
      const stacks = [...new Set(g.items.map(e => e.stack))];
      return `<tr class="group"><td style="width:30px"><input type="checkbox" data-pj="${esc(g.path)}" ${allSel ? 'checked' : ''}></td>
        <td colspan="2"><span class="lnk" data-col="${esc(g.path)}">${col ? '▸' : '▾'}</span> <b>${esc(g.p.name)}</b>
          ${stacks.map(s => `<span class="tag info">${esc(s)}</span>`).join(' ')} ${gitBadge(g.p.git)}
          <span class="tag ${stale ? 'keep' : 'safe'}" title="Newest source change or git commit">${d === null ? 'age unknown' : stale ? 'inactive ' + d + ' d' : 'active ' + d + ' d ago'}</span>
          <div class="muted path" style="font-weight:400"><code>${esc(g.path)}</code></div></td><td class="num">${fmt(g.total)}</td></tr>
        ${col ? '' : g.items.map(e => `<tr title="${esc(e.note)}"><td style="width:30px"></td>
          <td><input type="checkbox" data-jk="${esc(e.path)}" ${S.junkSel.has(e.path) ? 'checked' : ''}> <b>${esc(e.name)}</b> <span class="tag ${e.risk === 'safe' ? 'safe' : 'review'}">${esc(e.kind)}</span>
            ${e.note ? `<div class="muted" style="margin-left:22px">${esc(e.note)}</div>` : ''}</td>
          <td><code>${esc(e.rebuild)}</code> <span class="tag ${COST_TAG[e.cost] || 'info'}">${COST_LBL[e.cost] || ''}</span>
            ${e.debug_size > 0 && e.debug_size < e.size ? `<div style="margin-top:4px"><button class="btn sm" data-dbg="${esc(e.path)}" data-dbgsize="${e.debug_size}">Only target\\debug (${fmt(e.debug_size)})</button> <span class="muted">keeps release builds</span></div>` : ''}</td>
          <td class="num">${fmt(e.size)}<div class="muted">${e.is_file ? 'file' : num(e.files) + ' files'}</div></td></tr>`).join('')}`;
    }).join('')}</tbody></table>${note}<div id="jkSel"></div>`;

  const byPath = p => r.entries.find(e => e.path === p);
  const upd = () => selBar($('#jkSel'), S.junkSel, p => (byPath(p) || {}).size,
    async () => {
      const items = [...S.junkSel].map(byPath).filter(Boolean);
      if (await jkRemoveFlow(items)) {
        const gone = new Set(items.map(i => i.path));
        r.entries = r.entries.filter(e => !gone.has(e.path));
        S.junkSel = new Set(); renderJunk();
      }
    });
  $$('input[data-jk]', box).forEach(i => i.onchange = () => { i.checked ? S.junkSel.add(i.dataset.jk) : S.junkSel.delete(i.dataset.jk); upd(); renderJunkGroupBoxes(); });
  function renderJunkGroupBoxes() { $$('input[data-pj]', box).forEach(c => { const g = groups.find(x => x.path === c.dataset.pj); c.checked = g.items.every(e => S.junkSel.has(e.path)); }); }
  $$('input[data-pj]', box).forEach(c => c.onchange = () => {
    const g = groups.find(x => x.path === c.dataset.pj);
    g.items.forEach(e => c.checked ? S.junkSel.add(e.path) : S.junkSel.delete(e.path));
    renderJunk();
  });
  $$('[data-col]', box).forEach(a => a.onclick = () => { const p = a.dataset.col; S.junkCollapsed.has(p) ? S.junkCollapsed.delete(p) : S.junkCollapsed.add(p); renderJunk(); });
  $$('[data-stk]', box).forEach(c => c.onclick = () => {
    const k = c.dataset.stk;
    if (!k) S.junkStack.clear(); else S.junkStack.has(k) ? S.junkStack.delete(k) : S.junkStack.add(k);
    renderJunk();
  });
  $$('[data-dbg]', box).forEach(b => b.onclick = async () => {
    const e = byPath(b.dataset.dbg);
    const item = { path: e.path + '\\debug', size: +b.dataset.dbgsize, project: e.project, risk: 'safe', cost: 'high', kind: 'Rust target\\debug', rebuild: 'cargo build' };
    if (await deleteFlow([item], 'junk', { defaultMode: 'permanent' })) { e.size = Math.max(0, e.size - item.size); e.debug_size = 0; renderJunk(); }
  });
  $('#jkRisk').onchange = e => { S.junkRisk = e.target.value; renderJunk(); };
  $('#jkDays').onchange = async e => {
    S.settings.junk_stale_days = Math.max(1, +e.target.value || 30);
    S.settings = await invoke('set_settings', { settings: S.settings }); renderJunk();
  };
  $('#jkSafe').onclick = () => { vis.forEach(e => { if (e.risk === 'safe') S.junkSel.add(e.path); }); renderJunk(); };
  $('#jkStale').onclick = () => { staleItems.forEach(e => S.junkSel.add(e.path)); renderJunk(); if (!staleItems.length) toast('No inactive projects with safe items.'); };
  $('#jkClear').onclick = () => { S.junkSel = new Set(); renderJunk(); };
  upd();
}

VIEWS.junk = async () => {
  const qp = await invoke('quick_paths');
  const drives = await invoke('list_drives').catch(() => []);
  if (!S.junkRoots) S.junkRoots = (S.settings.junk_roots && S.settings.junk_roots.length) ? [...S.settings.junk_roots] : [qp[0].path];
  $('#view').innerHTML = `<h2>Dev Junk</h2><div class="sub">Finds regenerable build output, caches and dependency folders inside your project folders: <code>node_modules</code>, <code>.next</code>, Rust <code>target</code>, Unity <code>Library</code>, Unreal <code>Intermediate</code>, Gradle, Flutter, .NET <code>bin/obj</code>, Python venv and caches, Blender backups and more. Each one is verified against a file next to it (package.json, Cargo.toml, .uproject ...) or against git's ignore rules. It also checks git for uncommitted work and project age. Your source files are never listed.</div>
    <div class="card"><div class="muted" style="margin-bottom:6px">Folders to scan, one per line</div>
      <textarea id="jkRoots" rows="3" style="width:100%">${esc(S.junkRoots.join('\n'))}</textarea>
      <div class="row" style="margin-top:8px"><span class="muted">Add:</span><div class="chips">
        ${qp.slice(0, 4).map(q => `<span class="chip" data-addroot="${esc(q.path)}">${esc(q.label)}</span>`).join('')}
        ${drives.map(d => `<span class="chip" data-addroot="${esc(d.letter)}\\">${esc(d.letter)}\\</span>`).join('')}</div>
        <span class="spacer"></span>
        <label class="muted"><input type="checkbox" id="jkIgn" ${S.junkIgnored ? 'checked' : ''}> also list git-ignored folders (marked review)</label>
        <button class="btn primary" id="jkGo">Search</button></div></div>
    <div id="jkBody" style="margin-top:14px">${S.junk ? '' : '<div class="empty">Choose the folders that hold your projects, then press <b>Search</b>. A scan of a whole drive is fine, system and tool folders are skipped.</div>'}</div>`;
  $$('[data-addroot]').forEach(c => c.onclick = () => {
    const ta = $('#jkRoots'); const cur = ta.value.split('\n').map(x => x.trim()).filter(Boolean);
    if (!cur.some(x => x.toLowerCase() === c.dataset.addroot.toLowerCase())) cur.push(c.dataset.addroot);
    ta.value = cur.join('\n');
  });
  $('#jkGo').onclick = async () => {
    S.junkRoots = $('#jkRoots').value.split('\n').map(x => x.trim()).filter(Boolean);
    S.junkIgnored = $('#jkIgn').checked;
    const r = await busy('Searching project folders', () => invoke('find_dev_junk', { roots: S.junkRoots, includeIgnored: S.junkIgnored })).catch(e => { toast(String(e), true); return null; });
    if (!r) return;
    S.junk = r; S.junkSel = new Set(); S.junkStack = new Set();
    S.junkCollapsed = new Set(Object.keys(r.projects).length > 15 ? Object.keys(r.projects) : []);
    renderJunk();
  };
  renderJunk();
};

// ------------------------------------------------------------------ shared: live repair runner

const READONLY_FIX = new Set(['net_info', 'recent_errors', 'dism_check', 'dism_scan', 'chkdsk_scan']);
const EXPLAINABLE = new Set(['net_info', 'recent_errors', 'sfc', 'dism_check', 'dism_scan', 'dism_restore', 'chkdsk_scan']);

async function runFixUI(id, title, explainable) {
  const reqId = 'f' + Date.now();
  const lines = [];
  let logEl = null, stateEl = null, stopEl = null, aiEl = null, finished = false, doneInfo = null;
  const paint = () => {
    if (logEl) { logEl.textContent = lines.join('\n'); logEl.scrollTop = logEl.scrollHeight; }
    if (!stateEl) return;
    if (!finished) { stateEl.textContent = 'Running… keep this window open.'; return; }
    const d = doneInfo || {};
    stateEl.textContent = d.cancelled ? 'Stopped.' : d.error ? 'Could not run: ' + d.error : `Finished (exit code ${d.code}).` + (d.reboot ? ' Restart Windows to finish.' : '');
    if (stopEl) stopEl.classList.add('hidden');
    if (aiEl && explainable && lines.length) aiEl.classList.remove('hidden');
  };
  const u1 = await listen('fix-line', e => { if (e.payload.id === reqId) { lines.push(e.payload.line); if (lines.length > 4000) lines.shift(); paint(); } });
  const u2 = await listen('fix-done', e => { if (e.payload.id === reqId) { finished = true; doneInfo = e.payload; paint(); refreshDrive(); } });
  const mp = modal({
    title: esc(title),
    body: '<div class="muted" id="fxState">Starting…</div><pre class="term" id="fxLog"></pre>',
    buttons: [
      { label: 'Stop', value: 'stop', cls: 'danger', keep: true, onClick: () => { invoke('cancel_fix', { reqId }); } },
      { label: 'Explain with AI', value: 'ai', cls: 'hidden-ai' },
      { label: 'Close', value: 'close', cls: 'primary' },
    ],
    onOpen: root => {
      logEl = $('#fxLog', root); stateEl = $('#fxState', root);
      const bs = $$('.mfoot button', root);
      stopEl = bs[0]; aiEl = bs[1]; aiEl.classList.add('hidden');
      paint();
    },
  });
  try { await invoke('run_fix', { id, reqId }); }
  catch (e) { finished = true; doneInfo = { error: String(e) }; paint(); }
  const r = await mp;
  u1(); u2();
  if (r.value === 'ai') {
    const text = lines.slice(-70).join('\n').slice(-6000);
    go('ai');
    setTimeout(() => sendChat(`I ran "${title}" on my Windows PC. Explain this output in plain language, tell me if anything is wrong, and what I should do next:\n\n${text}`), 150);
  }
}

// ------------------------------------------------------------------ System Health

const STATUS_COL = { ok: '#22c55e', warn: '#f59e0b', bad: '#ef4444', unknown: '#64748b' };
const fdur = s => { const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600); return d ? `${d}d ${h}h` : `${h}h ${Math.floor(s % 3600 / 60)}m`; };

function scoreRing(score, grade) {
  const col = score >= 90 ? '#22c55e' : score >= 75 ? '#2dd4bf' : score >= 55 ? '#f59e0b' : '#ef4444';
  return `<div class="ring" style="--p:${score};--c:${col}"><div><b>${score}</b><small>${esc(grade)}</small></div></div>`;
}

async function loadHealth() { S.health = await invoke('health_check'); return S.health; }

VIEWS.health = async () => {
  $('#view').innerHTML = '<h2>System Health</h2><div class="sub">Every point in the score comes from a check below. Nothing is guessed.</div><div class="empty">Checking disks, memory, security and startup…</div>';
  const r = await busy('Checking your PC', loadHealth).catch(e => { toast(String(e), true); return null; });
  if (!r || S.page !== 'health') return;
  await refreshBadges();
  const vols = (r.volumes || []).filter(v => v.type === 'Fixed' && v.fs);
  $('#view').innerHTML = `<h2>System Health</h2><div class="sub">Every point in the score comes from a check below. Nothing is guessed.</div>${adminBanner()}
    <div class="grid g2">
      <div class="card row" style="gap:22px">${scoreRing(r.score, r.grade)}
        <div><div class="lbl">PC health score</div><div class="muted">${r.checks.filter(c => c.status === 'bad').length} problem(s) · ${r.checks.filter(c => c.status === 'warn').length} warning(s)</div>
          <div class="kv" style="margin-top:10px"><div>Memory</div><div>${fmt(r.ram_used)} of ${fmt(r.ram_total)}</div><div>CPU threads</div><div>${r.cpu_threads}</div><div>Uptime</div><div>${fdur(r.uptime_s)}</div><div>Startup items</div><div>${r.startup_enabled} enabled</div></div></div></div>
      <div class="card"><h3>Physical disks</h3>${(r.disks || []).length ? r.disks.map(d => `<div style="margin:8px 0"><b>${esc(d.name)}</b> <span class="tag ${d.health === 'Healthy' ? 'safe' : 'keep'}">${esc(d.health || '?')}</span> <span class="tag info">${esc(d.media)} · ${esc(d.bus)}</span>
        <div class="muted">${fmt(d.size)}${d.temp != null ? ' · ' + d.temp + ' °C' : ''}${d.wear != null ? ' · wear ' + d.wear + '%' : ''}${d.hours != null ? ' · ' + num(d.hours) + ' h powered on' : ''}${d.readErr || d.writeErr ? ' · errors logged' : ''}</div></div>`).join('') : '<div class="muted">Windows returned no disk data. Some counters need Administrator rights.</div>'}</div>
    </div>
    <h3 style="margin-top:18px">Checks</h3>
    <table><thead><tr><th style="width:26px"></th><th>Check</th><th>Result</th><th class="num">Points</th><th></th></tr></thead><tbody>
    ${r.checks.map(c => `<tr><td><i class="dotc" style="background:${STATUS_COL[c.status]}"></i></td><td><b>${esc(c.title)}</b></td><td class="muted">${esc(c.detail)}</td><td class="num">${c.max ? c.points + ' / ' + c.max : '–'}</td>
      <td class="num">${c.action && c.status !== 'ok' ? `<button class="btn sm" data-go="${esc(c.action)}">${c.action === 'startup' ? 'Open Startup' : c.action === 'clean' ? 'Open Safe Clean' : 'Details below'}</button>` : ''}</td></tr>`).join('')}</tbody></table>
    <h3 style="margin-top:22px">Drives</h3>
    <div class="muted" style="margin-bottom:8px">SSDs are kept healthy with TRIM, hard disks with defragmentation. Never defragment an SSD.${S.admin ? '' : ' These actions need Administrator rights.'}</div>
    <table><thead><tr><th>Drive</th><th>Type</th><th class="num">Free</th><th>Maintenance</th></tr></thead><tbody>
    ${vols.map(v => `<tr><td><b>${esc(v.letter)}:</b> <span class="muted">${esc(v.label || '')} ${esc(v.fs)}</span></td><td><span class="tag ${v.media === 'SSD' ? 'safe' : v.media === 'HDD' ? 'review' : 'info'}">${esc(v.media)}</span></td>
      <td class="num">${fmt(v.free)} <span class="muted">/ ${fmt(v.size)}</span></td>
      <td>${v.media === 'SSD' ? `<button class="btn sm" data-fx="trim:${esc(v.letter)}">TRIM</button> ` : ''}<button class="btn sm" data-fx="analyze:${esc(v.letter)}">Analyze</button>${v.media === 'HDD' ? ` <button class="btn sm" data-fx="defrag:${esc(v.letter)}">Defragment…</button>` : ''}</td></tr>`).join('')}</tbody></table>`;
  bindAdmin();
  $$('[data-go]').forEach(b => b.onclick = () => { if (b.dataset.go === 'health') $('#view').scrollTop = 99999; else go(b.dataset.go); });
  $$('[data-fx]').forEach(b => b.onclick = async () => {
    const id = b.dataset.fx;
    if (id.startsWith('defrag:')) {
      const c = await modal({ title: 'Defragment this drive?', body: 'This rearranges files on a hard disk and can run for hours. The PC stays usable but slower. Do not run it on an SSD.', buttons: [{ label: 'Cancel', value: false }, { label: 'Start', value: true, cls: 'primary' }] });
      if (!c.value) return;
    }
    runFixUI(id, b.textContent.trim() + ' · drive ' + id.split(':')[1]);
  });
};

// ------------------------------------------------------------------ Troubleshoot

VIEWS.fix = async () => {
  const defs = await invoke('list_fixes');
  await refreshBadges();
  const groups = [...new Set(defs.map(d => d.group))];
  $('#view').innerHTML = `<h2>Troubleshoot</h2><div class="sub">Well-known Windows repair commands, each explained, with live output. Only these built-in actions can run. After a run, <b>Explain with AI</b> turns the output into plain language.</div>${adminBanner()}
    ${groups.map(g => `<h3 style="margin:18px 0 8px">${esc(g)}</h3><div class="grid g2">${defs.filter(d => d.group === g).map(d => `<div class="card"><div class="row"><b>${esc(d.title)}</b><span class="spacer"></span>
      ${d.admin ? '<span class="tag review">Administrator</span>' : ''}${d.long ? '<span class="tag info">takes a while</span>' : ''}${d.reboot ? '<span class="tag info">needs restart</span>' : ''}${d.caution ? '<span class="tag keep">brief disruption</span>' : ''}</div>
      <div class="muted" style="margin:6px 0 10px">${esc(d.desc)}</div><button class="btn ${READONLY_FIX.has(d.id) ? '' : 'primary'}" data-run="${esc(d.id)}">Run</button></div>`).join('')}</div>`).join('')}`;
  bindAdmin();
  $$('[data-run]').forEach(b => b.onclick = async () => {
    const d = defs.find(x => x.id === b.dataset.run);
    if (d.admin && !S.admin) { toast('This needs Administrator rights. Use Restart as Administrator.', true); return; }
    if (!READONLY_FIX.has(d.id)) {
      const c = await modal({ title: esc(d.title) + '?', body: `<div>${esc(d.desc)}</div>${d.caution ? '<div class="banner warn" style="margin-top:10px"><span>Your screen or connection may flicker for a moment. Save your work first.</span></div>' : ''}${d.reboot ? '<div class="muted" style="margin-top:10px">A restart is needed afterwards.</div>' : ''}`, buttons: [{ label: 'Cancel', value: false }, { label: 'Run', value: true, cls: 'primary' }] });
      if (!c.value) return;
    }
    runFixUI(d.id, d.title, EXPLAINABLE.has(d.id));
  });
};

// ------------------------------------------------------------------ Startup

const KIND_LBL = { run: 'Registry', folder: 'Startup folder', task: 'Scheduled tasks', service: 'Services' };
VIEWS.startup = async () => {
  $('#view').innerHTML = '<h2>Startup</h2><div class="sub">Loading startup entries…</div>';
  const items = S.startup = await busy('Reading startup entries', () => invoke('list_startup')).catch(e => { toast(String(e), true); return []; });
  await refreshBadges();
  if (S.page !== 'startup') return;
  S.startFilter = S.startFilter || 'run';
  const draw = () => {
    const q = ($('#stQ') || {}).value || '';
    const vis = items.filter(i => (S.startFilter === 'all' || i.kind === S.startFilter) && (!q || (i.name + ' ' + i.command).toLowerCase().includes(q.toLowerCase())));
    const apps = items.filter(i => i.kind !== 'service');
    $('#stBody').innerHTML = `<table><thead><tr><th style="width:56px">On</th><th>Item</th><th>Where</th><th></th></tr></thead><tbody>
      ${vis.map(i => `<tr><td><label class="switch"><input type="checkbox" data-id="${esc(i.id)}" ${i.enabled ? 'checked' : ''}><i></i></label></td>
        <td><b>${esc(i.name)}</b> ${i.exists ? '' : '<span class="tag keep" title="The program this entry starts no longer exists">file missing</span>'}
          <div class="muted path"><code>${esc(i.command)}</code></div>${i.detail ? `<div class="muted">${esc(i.detail)}</div>` : ''}</td>
        <td class="muted">${esc(i.source)}${i.needs_admin && !S.admin ? ' <span class="tag review">needs Administrator</span>' : ''}</td>
        <td class="num"><button class="btn sm" data-ai="${esc(i.id)}">AI: explain</button></td></tr>`).join('') || '<tr><td colspan="4" class="empty">Nothing here.</td></tr>'}</tbody></table>`;
    $$('#stBody input[data-id]').forEach(c => c.onchange = async () => {
      const it = items.find(x => x.id === c.dataset.id);
      if (it.kind === 'service' && !c.checked && /security/.test(it.detail)) {
        const ok = await modal({ title: 'Disable a security service?', body: 'This looks like security software. Disabling it leaves the PC less protected.', buttons: [{ label: 'Cancel', value: false }, { label: 'Disable anyway', value: true, cls: 'danger' }] });
        if (!ok.value) { c.checked = true; return; }
      }
      c.disabled = true;
      try { await invoke('set_startup', { id: it.id, enable: c.checked }); it.enabled = c.checked; toast(`${it.name}: ${c.checked ? 'enabled' : 'disabled'}. Takes effect at the next sign-in.`); }
      catch (e) { c.checked = !c.checked; toast(String(e), true); }
      c.disabled = false; head();
    });
    $$('#stBody [data-ai]').forEach(b => b.onclick = async () => {
      if (!S.ollama.running) { toast('Ollama is not running.', true); return; }
      const it = items.find(x => x.id === b.dataset.ai);
      try {
        const r = await busy('Asking ' + S.settings.model, () => invoke('ai_explain_startup', { name: it.name, command: it.command, kind: it.source, model: S.settings.model, lang: S.settings.lang }));
        await modal({ title: `${esc(it.name)} <span class="tag ai">AI suggestion · verify first</span>`, body: `<div class="kv"><div>What it is</div><div>${esc(r.what)}</div><div>If disabled</div><div>${esc(r.impact)}</div><div>Suggestion</div><div><span class="tag ${r.advice === 'disable' ? 'review' : r.advice === 'keep' ? 'safe' : 'info'}">${esc(r.advice)}</span> ${esc(r.reason)}</div></div>`, buttons: [{ label: 'Close', value: true, cls: 'primary' }] });
      } catch (e) { toast(String(e), true); }
    });
  };
  const head = () => {
    const apps = items.filter(i => i.kind !== 'service');
    $('#stHead').innerHTML = `<div class="grid g4">
      <div class="card"><div class="lbl">Enabled at sign-in</div><div class="big">${apps.filter(i => i.enabled).length}</div><div class="muted">registry, folders, tasks</div></div>
      <div class="card"><div class="lbl">Disabled by you</div><div class="big">${apps.filter(i => !i.enabled).length}</div><div class="muted">switch back on any time</div></div>
      <div class="card"><div class="lbl">Broken entries</div><div class="big">${items.filter(i => !i.exists).length}</div><div class="muted">point to a missing program</div></div>
      <div class="card"><div class="lbl">Third-party services</div><div class="big">${items.filter(i => i.kind === 'service' && i.enabled).length}</div><div class="muted">set to start automatically</div></div></div>`;
  };
  $('#view').innerHTML = `<h2>Startup</h2><div class="sub">Programs that launch with Windows slow every boot. Switching one off does not uninstall it and is reversible. It uses the same switch as Task Manager. ${S.admin ? '' : 'All-users entries and services need Administrator rights.'}</div>${adminBanner()}
    <div id="stHead"></div>
    <div class="row" style="margin:14px 0 8px"><div class="chips">${['run', 'folder', 'task', 'service', 'all'].map(k => `<span class="chip ${S.startFilter === k ? 'on' : ''}" data-k="${k}">${k === 'all' ? 'All' : KIND_LBL[k]} · ${k === 'all' ? items.length : items.filter(i => i.kind === k).length}</span>`).join('')}</div><span class="spacer"></span><input type="text" id="stQ" placeholder="Filter…" style="width:220px"></div>
    <div id="stBody"></div>`;
  bindAdmin(); head(); draw();
  $('#stQ').oninput = draw;
  $$('[data-k]').forEach(c => c.onclick = () => { S.startFilter = c.dataset.k; VIEWS.startup.redraw(); });
  VIEWS.startup.redraw = () => { $$('[data-k]').forEach(c => c.classList.toggle('on', c.dataset.k === S.startFilter)); draw(); };
};

// ------------------------------------------------------------------ Duplicates

function dupSelAuto(g, how) {
  const files = [...g.files];
  let keep;
  if (how === 'oldest') keep = files.reduce((a, b) => (b.modified < a.modified ? b : a));
  else if (how === 'newest') keep = files.reduce((a, b) => (b.modified > a.modified ? b : a));
  else keep = files.reduce((a, b) => (b.path.length < a.path.length ? b : a));
  files.forEach(f => f.path === keep.path ? S.dupSel.delete(f.path) : S.dupSel.add(f.path));
}

function renderDupes() {
  const box = $('#dpBody');
  if (!box || !S.dup) return;
  const r = S.dup;
  if (!r.groups.length) { box.innerHTML = `<div class="empty">No identical files found among ${num(r.scanned_files)} files.</div>`; return; }
  const shown = r.groups.slice(0, S.dupShown);
  const sizeOf = p => { for (const g of r.groups) if (g.files.some(f => f.path === p)) return g.size; return 0; };
  box.innerHTML = `<div class="grid g4">
      <div class="card"><div class="lbl">Duplicate groups</div><div class="big">${num(r.groups.length)}${r.truncated ? '+' : ''}</div><div class="muted">byte-for-byte identical</div></div>
      <div class="card"><div class="lbl">Wasted space</div><div class="big">${fmt(r.total_wasted)}</div><div class="muted">if only one copy of each stays</div></div>
      <div class="card"><div class="lbl">Files compared</div><div class="big">${num(r.scanned_files)}</div><div class="muted">${num(r.same_size_files)} shared a size</div></div>
      <div class="card"><div class="lbl">Selected</div><div class="big">${S.dupSel.size}</div><div class="muted">${fmt([...S.dupSel].reduce((s, p) => s + sizeOf(p), 0))}</div></div></div>
    <div class="row" style="margin:14px 0"><span class="muted">Auto-select, keeping in every group the</span>
      <button class="btn sm" data-auto="oldest">oldest file</button><button class="btn sm" data-auto="newest">newest file</button><button class="btn sm" data-auto="shortest">shortest path</button><button class="btn sm" data-auto="none">Clear</button></div>
    ${shown.map((g, gi) => `<div class="card" style="margin-bottom:10px"><div class="row"><b>${fmt(g.size)}</b><span class="muted">× ${g.files.length} copies · ${fmt(g.wasted)} wasted</span></div>
      <table><tbody>${g.files.map(f => `<tr><td style="width:30px"><input type="checkbox" data-g="${gi}" data-p="${esc(f.path)}" ${S.dupSel.has(f.path) ? 'checked' : ''}></td>
        <td class="path"><code>${esc(f.path)}</code></td><td class="num muted">${fdate(f.modified)}</td><td class="num"><span class="lnk" data-rv="${esc(f.path)}">Show</span></td></tr>`).join('')}</tbody></table></div>`).join('')}
    ${r.groups.length > shown.length ? `<div class="row" style="justify-content:center;margin:10px"><button class="btn" id="dpMore">Show ${Math.min(100, r.groups.length - shown.length)} more groups</button></div>` : ''}<div id="dpSel"></div>`;
  $$('[data-auto]').forEach(b => b.onclick = () => {
    if (b.dataset.auto === 'none') S.dupSel = new Set(); else shown.forEach(g => dupSelAuto(g, b.dataset.auto));
    renderDupes();
  });
  $$('input[data-p]').forEach(c => c.onchange = () => {
    const g = r.groups[+c.dataset.g];
    if (c.checked) {
      S.dupSel.add(c.dataset.p);
      if (g.files.every(f => S.dupSel.has(f.path))) { S.dupSel.delete(c.dataset.p); toast('Keep at least one copy in every group.', true); }
    } else S.dupSel.delete(c.dataset.p);
    renderDupes();
  });
  $$('[data-rv]').forEach(a => a.onclick = () => invoke('reveal_in_explorer', { path: a.dataset.rv }));
  const more = $('#dpMore'); if (more) more.onclick = () => { S.dupShown += 100; renderDupes(); };
  selBar($('#dpSel'), S.dupSel, sizeOf, async () => {
    const paths = [...S.dupSel];
    const groups = r.groups.map(g => ({ keep: g.files.filter(f => !S.dupSel.has(f.path)).map(f => f.path), remove: g.files.filter(f => S.dupSel.has(f.path)).map(f => f.path) })).filter(g => g.remove.length);
    const items = paths.map(p => ({ path: p, size: sizeOf(p) }));
    const ok = await deleteFlow(items, null, {
      extraHtml: '<div class="muted" style="margin-bottom:8px">Each file is compared with the copy that stays once more right before removal. A file that changed since the scan is skipped.</div>',
      remove: mode => invoke('delete_duplicates', { groups, mode }),
    });
    if (ok) {
      const gone = new Set(paths);
      r.groups.forEach(g => { g.files = g.files.filter(f => !gone.has(f.path)); g.wasted = g.size * Math.max(0, g.files.length - 1); });
      S.dup.groups = r.groups.filter(g => g.files.length > 1); S.dup.total_wasted = S.dup.groups.reduce((s, g) => s + g.wasted, 0);
      S.dupSel = new Set(); renderDupes();
    }
  });
}

VIEWS.dupes = async () => {
  const qp = await invoke('quick_paths');
  const drives = await invoke('list_drives').catch(() => []);
  if (!S.dupRoots) S.dupRoots = [qp[1].path, qp[2].path, qp[3].path];
  $('#view').innerHTML = `<h2>Duplicates</h2><div class="sub">Finds files with identical content, whatever their names. Files are matched by size, then a quick fingerprint, then a full BLAKE3 hash, so only true byte-for-byte copies are listed. Windows, Program Files, AppData and tool caches are skipped, and cloud-only OneDrive files are never downloaded.</div>
    <div class="card"><div class="muted" style="margin-bottom:6px">Folders to scan, one per line</div><textarea id="dpRoots" rows="3" style="width:100%">${esc(S.dupRoots.join('\n'))}</textarea>
      <div class="row" style="margin-top:8px"><span class="muted">Add:</span><div class="chips">${qp.slice(0, 4).map(q => `<span class="chip" data-addroot="${esc(q.path)}">${esc(q.label)}</span>`).join('')}${drives.map(d => `<span class="chip" data-addroot="${esc(d.letter)}\\">${esc(d.letter)}\\</span>`).join('')}</div>
        <span class="spacer"></span><label class="muted">Ignore files smaller than <input type="number" id="dpMin" min="0.1" step="0.5" value="${S.dupMin}" style="width:70px"> MB</label><button class="btn primary" id="dpGo">Find duplicates</button></div></div>
    <div id="dpBody" style="margin-top:14px">${S.dup ? '' : '<div class="empty">Choose folders and press <b>Find duplicates</b>.</div>'}</div>`;
  $$('[data-addroot]').forEach(c => c.onclick = () => {
    const ta = $('#dpRoots'); const cur = ta.value.split('\n').map(x => x.trim()).filter(Boolean);
    if (!cur.some(x => x.toLowerCase() === c.dataset.addroot.toLowerCase())) cur.push(c.dataset.addroot);
    ta.value = cur.join('\n');
  });
  $('#dpGo').onclick = async () => {
    S.dupRoots = $('#dpRoots').value.split('\n').map(x => x.trim()).filter(Boolean);
    S.dupMin = Math.max(0.1, +$('#dpMin').value || 1);
    const r = await busy('Comparing files', () => invoke('find_duplicates', { roots: S.dupRoots, minMb: S.dupMin })).catch(e => { toast(String(e), true); return null; });
    if (!r) return;
    S.dup = r; S.dupSel = new Set(); S.dupShown = 100; renderDupes();
  };
  renderDupes();
};

// ------------------------------------------------------------------ Recover Files

async function recycleTab() {
  const box = $('#rcBody');
  box.innerHTML = '<div class="empty">Reading the Recycle Bin…</div>';
  const items = await busy('Reading the Recycle Bin', () => invoke('recycle_list')).catch(e => { box.innerHTML = `<div class="empty">${esc(e)}</div>`; return null; });
  if (!items) return;
  S.bin = items; S.binSel = new Set();
  const draw = () => {
    const q = (($('#rbQ') || {}).value || '').toLowerCase();
    const vis = S.bin.filter(i => !q || (i.name + ' ' + i.orig).toLowerCase().includes(q));
    box.innerHTML = `<div class="row" style="margin-bottom:8px"><b>${S.bin.length} item(s) · ${fmt(S.bin.reduce((s, i) => s + i.size, 0))}</b><span class="spacer"></span><input type="text" id="rbQ" placeholder="Search name or folder…" value="${esc(q)}" style="width:240px"><button class="btn" id="rbAll">Select shown</button></div>
      ${vis.length ? `<table><thead><tr><th></th><th>Name</th><th>Original folder</th><th>Deleted</th><th class="num">Size</th></tr></thead><tbody>
      ${vis.slice(0, 400).map(i => `<tr><td style="width:30px"><input type="checkbox" data-rb="${esc(i.path)}" ${S.binSel.has(i.path) ? 'checked' : ''}></td><td><b>${esc(i.name)}</b>${i.isdir ? ' <span class="tag info">folder</span>' : ''}</td><td class="path muted">${esc(i.orig)}</td><td class="muted">${esc(i.deleted)}</td><td class="num">${fmt(i.size)}</td></tr>`).join('')}</tbody></table>${vis.length > 400 ? `<div class="muted" style="margin:8px">Showing 400 of ${vis.length}. Narrow the search.</div>` : ''}` : '<div class="empty">The Recycle Bin is empty.</div>'}<div id="rbSel"></div>`;
    $('#rbQ').oninput = e => { const v = e.target.value; draw(); const el = $('#rbQ'); el.focus(); el.setSelectionRange(v.length, v.length); };
    $('#rbAll').onclick = () => { vis.slice(0, 400).forEach(i => S.binSel.add(i.path)); draw(); };
    $$('input[data-rb]').forEach(c => c.onchange = () => { c.checked ? S.binSel.add(c.dataset.rb) : S.binSel.delete(c.dataset.rb); upd(); });
    const upd = () => {
      const el = $('#rbSel');
      if (!S.binSel.size) { el.innerHTML = ''; return; }
      el.innerHTML = `<div class="sel-bar"><b>${S.binSel.size} selected</b><span class="spacer"></span><button class="btn primary" id="rbGo">Restore to original location</button></div>`;
      $('#rbGo').onclick = async () => {
        const paths = [...S.binSel];
        const r = await busy('Restoring', () => invoke('recycle_restore', { paths })).catch(e => { toast(String(e), true); return null; });
        if (!r) return;
        toast(`${r.restored} of ${r.requested} restored` + (r.remaining ? ` · ${r.remaining} could not be restored (name conflict or missing folder)` : ''), r.remaining > 0);
        recycleTab(); refreshDrive();
      };
    };
    upd();
  };
  draw();
}

async function shadowTab() {
  const box = $('#rcBody');
  box.innerHTML = `<div class="muted" style="margin-bottom:10px">Windows keeps hidden snapshots of your drives (System Restore and backups). If one exists from before a file was deleted or changed, you can copy the old version out. Snapshots are only read, never changed. This only works when System Protection is on for that drive.</div>${S.admin ? '' : adminBanner()}<div id="shBody"><div class="empty">${S.admin ? 'Looking for snapshots…' : ''}</div></div>`;
  bindAdmin();
  if (!S.admin) return;
  const list = await busy('Looking for snapshots', () => invoke('shadow_list')).catch(e => { $('#shBody').innerHTML = `<div class="empty">${esc(e)}</div>`; return null; });
  if (!list) return;
  if (!list.length) { $('#shBody').innerHTML = '<div class="empty">No snapshots exist on this PC. Turn on System Protection (System Properties → System Protection) to start keeping them.</div>'; return; }
  const sys = S.sysDrive.replace(':', '');
  S.shPath = S.shPath || (await invoke('quick_paths'))[0].path;
  $('#shBody').innerHTML = `<div class="row"><label class="muted">Snapshot <select id="shSel">${list.map(s => `<option value="${esc(s.device)}">${esc(s.letter || '?')}: · ${esc(fdt(s.created))}</option>`).join('')}</select></label>
      <input type="text" id="shPath" style="flex:1;min-width:260px" value="${esc(S.shPath)}"><button class="btn primary" id="shGo">Browse</button></div><div id="shList" style="margin-top:12px"></div>`;
  const browse = async (path) => {
    S.shPath = path; $('#shPath').value = path;
    const m = path.match(/^([A-Za-z]):\\?(.*)$/);
    if (!m) { toast('Enter a full path such as C:\\Users\\you\\Documents', true); return; }
    const dev = $('#shSel').value, sel = list.find(s => s.device === dev);
    if (sel && sel.letter && sel.letter.toUpperCase() !== m[1].toUpperCase()) toast(`This snapshot is of drive ${sel.letter}:, not ${m[1].toUpperCase()}:`, true);
    const rows = await busy('Reading the snapshot', () => invoke('shadow_browse', { device: dev, path: m[2] })).catch(e => { toast(String(e), true); return null; });
    if (!rows) return;
    const sel2 = new Set();
    const base = path.replace(/\\+$/, '');
    $('#shList').innerHTML = `<div class="muted" style="margin-bottom:6px">${esc(base)}\\ · ${rows.length} entries</div>
      <table><thead><tr><th></th><th>Name</th><th class="num">Size</th><th class="num">Modified</th></tr></thead><tbody>
      ${base.length > 3 ? '<tr><td></td><td><span class="lnk" data-up="1">⬑ Up</span></td><td></td><td></td></tr>' : ''}
      ${rows.map((r, i) => `<tr><td style="width:30px"><input type="checkbox" data-sh="${i}"></td><td>${r.is_dir ? `<span class="lnk" data-dir="${esc(r.name)}">📁 ${esc(r.name)}</span>` : esc(r.name)}</td><td class="num">${r.is_dir ? '' : fmt(r.size)}</td><td class="num muted">${fdate(r.modified)}</td></tr>`).join('')}</tbody></table><div id="shAct"></div>`;
    $$('[data-dir]').forEach(a => a.onclick = () => browse(base + '\\' + a.dataset.dir));
    const up = $('[data-up]'); if (up) up.onclick = () => browse(base.replace(/\\[^\\]*$/, '') || base.slice(0, 3));
    $$('input[data-sh]').forEach(c => c.onchange = () => {
      c.checked ? sel2.add(+c.dataset.sh) : sel2.delete(+c.dataset.sh);
      const el = $('#shAct');
      if (!sel2.size) { el.innerHTML = ''; return; }
      el.innerHTML = `<div class="sel-bar"><b>${sel2.size} selected</b><span class="spacer"></span><button class="btn primary" id="shRestore">Copy out…</button></div>`;
      $('#shRestore').onclick = async () => {
        const dflt = (await invoke('quick_paths'))[1].path + '\\Recovered files';
        const r = await modal({ title: 'Copy old versions to', body: `<input type="text" name="dest" style="width:100%" value="${esc(dflt)}"><div class="muted" style="margin-top:8px">Nothing is overwritten. If a name already exists, the copy gets a "(recovered n)" suffix.</div>`, buttons: [{ label: 'Cancel', value: false }, { label: 'Copy', value: true, cls: 'primary' }] });
        if (!r.value) return;
        const rels = [...sel2].map(i => (m[2] ? m[2].replace(/\\+$/, '') + '\\' : '') + rows[i].name);
        const res = await busy('Copying', () => invoke('shadow_restore', { device: dev, paths: rels, dest: r.data.dest })).catch(e => { toast(String(e), true); return null; });
        if (!res) return;
        await alertBox('Copied', `<b>${res.files}</b> file(s) · ${fmt(res.bytes)} copied to <code>${esc(res.dest)}</code>${res.errors.length ? `<div style="margin-top:8px" class="muted">${res.errors.slice(0, 5).map(esc).join('<br>')}</div>` : ''}`);
        invoke('reveal_in_explorer', { path: res.dest });
      };
    });
  };
  $('#shGo').onclick = () => browse($('#shPath').value.trim());
}

VIEWS.recover = async () => {
  await refreshBadges();
  S.rcTab = S.rcTab || 'bin';
  $('#view').innerHTML = `<h2>Recover Files</h2><div class="sub">Three safe layers, from easiest to deepest. Files removed through this app are in <b>Quarantine & History</b> and can be restored there.</div>
    <div class="chips" style="margin-bottom:14px"><span class="chip ${S.rcTab === 'bin' ? 'on' : ''}" data-t="bin">Recycle Bin</span><span class="chip ${S.rcTab === 'shadow' ? 'on' : ''}" data-t="shadow">Previous versions</span><span class="chip ${S.rcTab === 'deep' ? 'on' : ''}" data-t="deep">Deleted for good?</span><span class="chip" data-t="q">Quarantine →</span></div><div id="rcBody"></div>`;
  $$('[data-t]').forEach(c => c.onclick = () => { if (c.dataset.t === 'q') { go('quarantine'); return; } S.rcTab = c.dataset.t; VIEWS.recover(); });
  if (S.rcTab === 'bin') recycleTab();
  else if (S.rcTab === 'shadow') shadowTab();
  else $('#rcBody').innerHTML = `<div class="card"><h3>If a file is gone from the Recycle Bin</h3>
    <div class="muted" style="line-height:1.6">Deleted files can often be carved back from the disk, but only until the space is reused. The odds depend on the drive:
    <table style="margin:10px 0"><thead><tr><th>Drive</th><th>Chance</th><th>Why</th></tr></thead><tbody>
    <tr><td>Hard disk (HDD)</td><td><span class="tag safe">good, if you act fast</span></td><td>Deleted data stays until overwritten.</td></tr>
    <tr><td>SSD / NVMe with TRIM</td><td><span class="tag keep">usually none</span></td><td>Windows tells the SSD to erase freed blocks within moments.</td></tr>
    <tr><td>USB stick / SD card</td><td><span class="tag review">often possible</span></td><td>Usually no TRIM.</td></tr></tbody></table>
    <b>What to do now:</b> stop using that drive, do not install recovery software onto it, and run Microsoft's free <code>winfr</code> (Windows File Recovery from the Microsoft Store) from another drive. Check <b>Previous versions</b> first: it needs no carving and works on SSDs too.</div></div>`;
};

// ------------------------------------------------------------------ Phase 2: batch runner

async function runFixBatch(ids, title, after) {
  const lines = [];
  let logEl = null, stateEl = null, stopEl = null, finished = false, cancelled = false, cur = null;
  const paint = msg => {
    if (logEl) { logEl.textContent = lines.join('\n'); logEl.scrollTop = logEl.scrollHeight; }
    if (stateEl && msg) stateEl.textContent = msg;
  };
  const waiters = {};
  const u1 = await listen('fix-line', e => { if (e.payload.id === cur) { lines.push(e.payload.line); if (lines.length > 4000) lines.shift(); paint(); } });
  const u2 = await listen('fix-done', e => { const w = waiters[e.payload.id]; if (w) w(e.payload); });
  const mp = modal({
    title: esc(title),
    body: '<div class="muted" id="fxState">Starting…</div><pre class="term" id="fxLog"></pre>',
    buttons: [
      { label: 'Stop', value: 'stop', cls: 'danger', keep: true, onClick: () => { cancelled = true; if (cur) invoke('cancel_fix', { reqId: cur }); } },
      { label: 'Close', value: 'close', cls: 'primary' },
    ],
    onOpen: root => { logEl = $('#fxLog', root); stateEl = $('#fxState', root); stopEl = $$('.mfoot button', root)[0]; paint(); },
  });
  let ok = 0, bad = 0;
  for (let i = 0; i < ids.length && !cancelled; i++) {
    const id = ids[i];
    cur = 'b' + Date.now() + '_' + i;
    lines.push(`── ${id.replace(/^(winget|debloat):/, '')} (${i + 1}/${ids.length}) ──`);
    paint(`Working on ${i + 1} of ${ids.length}… keep this window open.`);
    const done = new Promise(res => { waiters[cur] = res; });
    try { await invoke('run_fix', { id, reqId: cur }); } catch (e) { lines.push('Could not start: ' + e); bad++; continue; }
    const d = await done;
    if (d.error || (d.code !== 0 && d.code != null && !d.cancelled)) { bad++; lines.push(d.error ? 'Failed: ' + d.error : `Finished with exit code ${d.code}.`); } else ok++;
  }
  paint(`${cancelled ? 'Stopped. ' : ''}${ok} finished, ${bad} reported a problem.`);
  if (stopEl) stopEl.classList.add('hidden');
  const r = await mp; u1(); u2();
  if (after) after(ok, bad);
  return r;
}

// ------------------------------------------------------------------ Optimize

VIEWS.optimize = async () => {
  $('#view').innerHTML = '<h2>Optimize</h2><div class="sub">Reading current settings…</div>';
  const [t, plans, bloat] = await busy('Reading settings', () => Promise.all([invoke('list_tweaks'), invoke('list_power_plans').catch(() => []), invoke('list_bloat').catch(() => [])]));
  await refreshBadges();
  if (S.page !== 'optimize') return;
  S.bloatSel = new Set();
  const groups = [...new Set(t.tweaks.map(x => x.group))];
  $('#view').innerHTML = `<h2>Optimize</h2><div class="sub">Small, reversible changes. Each one says exactly what it changes. These reduce clutter, background activity and disk use; none of them is a magic speed boost, and the app does not claim one. Turning a switch off puts back the value Windows had before.</div>${adminBanner()}
    <div class="card row" style="gap:14px"><div style="flex:1"><b>Create a restore point first</b><div class="muted">One click saves a System Restore snapshot, so you can roll Windows settings back if you ever want to. Needs Administrator, and Windows allows one per 24 hours.</div></div><button class="btn primary" id="opRp">Create restore point</button></div>
    ${plans.length ? `<h3 style="margin:18px 0 8px">Power plan</h3><div class="card"><div class="chips">${plans.map(p => `<span class="chip ${p.active ? 'on' : ''}" data-plan="${esc(p.guid)}">${esc(p.name)}${p.active ? ' · active' : ''}</span>`).join('')}</div><div class="muted" style="margin-top:8px">Performance plans keep the CPU clocks higher and use more power. On a laptop that shortens battery life. Pick any plan to switch; pick the old one to switch back.</div></div>` : ''}
    ${groups.map(g => `<h3 style="margin:18px 0 8px">${esc(g)}</h3><div class="card" style="padding:4px 16px">${t.tweaks.filter(x => x.group === g).map(x => `<div class="row" style="padding:12px 0;border-bottom:1px solid var(--line);gap:14px">
      <label class="switch"><input type="checkbox" data-tw="${esc(x.id)}" ${x.applied ? 'checked' : ''}><i></i></label>
      <div style="flex:1"><b>${esc(x.title)}</b>${x.machine ? ' <span class="tag review">Administrator</span>' : ''}${x.id === 'hibernate' && t.hiber_bytes ? ` <span class="tag info">hiberfil.sys is ${fmt(t.hiber_bytes)}</span>` : ''}
      <div class="muted">${esc(x.desc)}</div>${x.caution ? `<div class="muted" style="color:var(--warn,#f59e0b)">⚠ ${esc(x.caution)}</div>` : ''}</div></div>`).join('')}</div>`).join('')}
    <h3 style="margin:22px 0 8px">Debloat: Microsoft Store apps</h3>
    <div class="muted" style="margin-bottom:8px">Only apps on a fixed list can be removed, and only for your account. Anything can be reinstalled from the Microsoft Store. Items marked <span class="tag keep">check first</span> are used by some people.</div>
    <div id="bloatBox"></div>`;
  bindAdmin();
  $('#opRp').onclick = () => {
    if (!S.admin) { toast('Creating a restore point needs Administrator rights.', true); return; }
    runFixUI('restore_point', 'Create a restore point');
  };
  $$('[data-plan]').forEach(c => c.onclick = async () => {
    try { await invoke('set_power_plan', { guid: c.dataset.plan }); toast('Power plan changed.'); VIEWS.optimize(); } catch (e) { toast(String(e), true); }
  });
  $$('input[data-tw]').forEach(c => c.onchange = async () => {
    const x = t.tweaks.find(y => y.id === c.dataset.tw);
    if (x.caution) {
      const ok = await modal({ title: esc(x.title), body: `<div>${esc(x.desc)}</div><div class="banner warn" style="margin-top:10px"><span>${esc(x.caution)}</span></div>`, buttons: [{ label: 'Cancel', value: false }, { label: c.checked ? 'Apply' : 'Undo', value: true, cls: 'primary' }] });
      if (!ok.value) { c.checked = !c.checked; return; }
    }
    c.disabled = true;
    try { await invoke('set_tweak', { id: x.id, on: c.checked }); toast(`${x.title}: ${c.checked ? 'applied' : 'undone'}.`); if (x.id === 'hibernate') { refreshDrive(); VIEWS.optimize(); return; } }
    catch (e) { c.checked = !c.checked; toast(String(e), true); }
    c.disabled = false;
  });
  const drawBloat = () => {
    const box = $('#bloatBox');
    if (!bloat.length) { box.innerHTML = '<div class="empty">None of the listed apps are installed.</div>'; return; }
    box.innerHTML = `<table><thead><tr><th style="width:30px"></th><th>App</th><th>Note</th></tr></thead><tbody>${bloat.map((b, i) => `<tr><td><input type="checkbox" data-bl="${i}" ${S.bloatSel.has(i) ? 'checked' : ''}></td><td><b>${esc(b.label)}</b> ${b.caution ? '<span class="tag keep">check first</span>' : ''}<div class="muted">${esc(b.name)}</div></td><td class="muted">${esc(b.note)}</td></tr>`).join('')}</tbody></table><div id="blSel"></div>`;
    $$('input[data-bl]').forEach(c => c.onchange = () => { c.checked ? S.bloatSel.add(+c.dataset.bl) : S.bloatSel.delete(+c.dataset.bl); upd(); });
    const upd = () => {
      const el = $('#blSel');
      if (!S.bloatSel.size) { el.innerHTML = ''; return; }
      el.innerHTML = `<div class="sel-bar"><b>${S.bloatSel.size} selected</b><span class="spacer"></span><button class="btn danger" id="blGo">Remove selected</button></div>`;
      $('#blGo').onclick = async () => {
        const picked = [...S.bloatSel].map(i => bloat[i]);
        const c = await modal({ title: `Remove ${picked.length} app(s)?`, body: `<ul class="plist">${picked.map(p => `<li>${esc(p.label)}</li>`).join('')}</ul><div class="muted">Removed for your account only. You can reinstall any of them from the Microsoft Store. Creating a restore point first is a good habit.</div>`, buttons: [{ label: 'Cancel', value: false }, { label: 'Remove', value: true, cls: 'danger' }] });
        if (!c.value) return;
        await runFixBatch(picked.map(p => 'debloat:' + p.name), 'Removing apps', () => VIEWS.optimize());
      };
    };
    upd();
  };
  drawBloat();
};

// ------------------------------------------------------------------ Programs (uninstaller)

VIEWS.apps = async () => {
  $('#view').innerHTML = '<h2>Programs</h2><div class="sub">Reading installed programs…</div>';
  const items = S.apps = await busy('Reading installed programs', () => invoke('list_installed')).catch(e => { toast(String(e), true); return []; });
  if (S.page !== 'apps') return;
  S.appSort = S.appSort || 'name';
  const draw = () => {
    const q = (($('#apQ') || {}).value || '').toLowerCase();
    let vis = items.filter(a => !q || (a.name + ' ' + a.publisher).toLowerCase().includes(q));
    const key = S.appSort;
    vis = [...vis].sort((a, b) => key === 'size' ? b.size - a.size : key === 'date' ? (b.installed || '').localeCompare(a.installed || '') : a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
    $('#apBody').innerHTML = `<table><thead><tr><th>Program</th><th>Publisher</th><th>Version</th><th>Installed</th><th class="num">Size</th><th></th></tr></thead><tbody>
      ${vis.map(a => `<tr><td><b>${esc(a.name)}</b></td><td class="muted">${esc(a.publisher)}</td><td class="muted">${esc(a.version)}</td><td class="muted">${esc(a.installed || '–')}</td><td class="num">${a.size ? fmt(a.size) : '–'}</td><td class="num"><button class="btn sm danger" data-un="${esc(a.id)}">Uninstall</button></td></tr>`).join('') || '<tr><td colspan="6" class="empty">No match.</td></tr>'}</tbody></table>`;
    $$('[data-un]').forEach(b => b.onclick = async () => {
      const a = items.find(x => x.id === b.dataset.un);
      const sec = /antivirus|security|defender|firewall|malware|endpoint/i.test(a.name);
      const c = await modal({ title: `Uninstall ${esc(a.name)}?`, body: `<div>This starts the program's own uninstaller${a.publisher ? ' (' + esc(a.publisher) + ')' : ''}. Its window may appear and may ask questions.</div>${sec ? '<div class="banner warn" style="margin-top:10px"><span>This looks like security software. Removing it can leave the PC unprotected.</span></div>' : ''}${a.quiet ? '<label class="check" style="margin-top:10px"><input type="checkbox" name="quiet"> Silent uninstall (no questions)</label>' : ''}`, buttons: [{ label: 'Cancel', value: false }, { label: 'Uninstall', value: true, cls: 'danger' }] });
      if (!c.value) return;
      try { await invoke('uninstall_app', { id: a.id, quiet: !!c.data.quiet }); toast('Uninstaller started. The list refreshes when it finishes.'); }
      catch (e) { toast(String(e), true); }
    });
  };
  $('#view').innerHTML = `<h2>Programs</h2><div class="sub">${items.length} installed programs. Uses each program's own uninstaller, the same one as Windows Settings. Size is what the program reports; it can be missing or approximate.</div>
    <div class="row" style="margin:10px 0"><div class="chips">${[['name', 'Name'], ['size', 'Largest'], ['date', 'Newest']].map(([k, l]) => `<span class="chip ${S.appSort === k ? 'on' : ''}" data-s="${k}">${l}</span>`).join('')}</div><span class="spacer"></span><input type="text" id="apQ" placeholder="Search…" style="width:240px"></div><div id="apBody"></div>`;
  draw();
  $('#apQ').oninput = draw;
  $$('[data-s]').forEach(c => c.onclick = () => { S.appSort = c.dataset.s; $$('[data-s]').forEach(x => x.classList.toggle('on', x === c)); draw(); });
  if (!S.uninstallBound) {
    S.uninstallBound = true;
    listen('uninstall-done', () => { refreshDrive(); if (S.page === 'apps') VIEWS.apps(); });
  }
};

// ------------------------------------------------------------------ Updates (winget)

VIEWS.updates = async () => {
  $('#view').innerHTML = '<h2>Updates</h2><div class="sub">Asking winget what has a newer version… this usually takes a few seconds, up to a minute when winget refreshes its package list.</div>';
  const res = await busy('Checking for updates', () => invoke('list_updates')).catch(e => {
    const msg = String(e), v6 = msg.startsWith('IPV6:');
    $('#view').innerHTML = `<h2>Updates</h2><div class="empty">${esc(msg.replace(/^(IPV6|TIMEOUT):/, ''))}<div style="margin-top:12px">${v6 ? '<button class="btn primary" id="upV4">Prefer IPv4 (fix)</button> ' : ''}<button class="btn" id="upRetry">Try again</button></div></div>`;
    $('#upRetry').onclick = () => VIEWS.updates();
    if (v6) $('#upV4').onclick = () => runFixBatch(['prefer_ipv4'], 'Preferring IPv4', () => VIEWS.updates());
    return null;
  });
  if (!res || S.page !== 'updates') return;
  const { items, note } = res;
  const sel = new Set();
  const draw = () => {
    $('#view').innerHTML = `<h2>Updates</h2><div class="sub">Program updates from winget, Microsoft's package manager. Installers come from winget's package sources. Only programs winget recognises are listed. Some updaters ask for Administrator approval or close the running program.</div>
      ${note ? `<div class="banner info">${esc(note)}</div>` : ''}<div class="row" style="margin:10px 0"><b>${items.length} update(s) available</b><span class="spacer"></span><button class="btn" id="upRe">Check again</button>${items.length ? '<button class="btn primary" id="upAll">Update all</button>' : ''}</div>
      ${items.length ? `<table><thead><tr><th style="width:30px"></th><th>Program</th><th>Installed</th><th>Available</th><th>Source</th></tr></thead><tbody>${items.map((u, i) => `<tr><td><input type="checkbox" data-u="${i}" ${sel.has(i) ? 'checked' : ''}></td><td><b>${esc(u.name)}</b><div class="muted">${esc(u.id)}</div></td><td class="muted">${esc(u.version)}</td><td>${esc(u.available)}</td><td class="muted">${esc(u.source)}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">Everything winget manages is up to date.</div>'}<div id="upSel"></div>`;
    $('#upRe').onclick = () => VIEWS.updates();
    const all = $('#upAll'); if (all) all.onclick = async () => {
      const c = await modal({ title: `Update all ${items.length} programs?`, body: '<div class="muted">Programs that are open may be closed by their updater. Save your work first.</div>', buttons: [{ label: 'Cancel', value: false }, { label: 'Update all', value: true, cls: 'primary' }] });
      if (c.value) await runFixBatch(['winget_all'], 'Updating all programs', () => VIEWS.updates());
    };
    $$('input[data-u]').forEach(c => c.onchange = () => {
      c.checked ? sel.add(+c.dataset.u) : sel.delete(+c.dataset.u);
      const el = $('#upSel');
      if (!sel.size) { el.innerHTML = ''; return; }
      el.innerHTML = `<div class="sel-bar"><b>${sel.size} selected</b><span class="spacer"></span><button class="btn primary" id="upGo">Update selected</button></div>`;
      $('#upGo').onclick = () => runFixBatch([...sel].map(i => 'winget:' + items[i].id), 'Updating programs', () => VIEWS.updates());
    });
  };
  draw();
};

// ------------------------------------------------------------------ Phase 3: Processes

const dayName = m => ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'].filter((_, i) => m & (1 << i)).join(', ');

VIEWS.procs = async () => {
  $('#view').innerHTML = '<h2>Processes</h2><div class="sub">Reading running programs…</div>';
  let list = [];
  const load = async () => { list = await invoke('list_processes'); };
  await busy('Reading processes', load).catch(e => toast(String(e), true));
  if (S.page !== 'procs') return;
  S.pGroup = S.pGroup !== false; S.pSort = S.pSort || 'mem';
  const rows = () => {
    const q = (($('#prQ') || {}).value || '').toLowerCase();
    let v = list.filter(p => !q || (p.name + ' ' + p.path).toLowerCase().includes(q));
    if (S.pGroup) {
      const m = new Map();
      v.forEach(p => { const k = p.name.toLowerCase(); const g = m.get(k) || { name: p.name, path: p.path, pids: [], cpu: 0, mem: 0 }; g.pids.push(p.pid); g.cpu += p.cpu; g.mem += p.mem; m.set(k, g); });
      v = [...m.values()];
    } else v = v.map(p => ({ name: p.name, path: p.path, pids: [p.pid], cpu: p.cpu, mem: p.mem }));
    return v.sort((a, b) => S.pSort === 'cpu' ? b.cpu - a.cpu : S.pSort === 'name' ? a.name.toLowerCase().localeCompare(b.name.toLowerCase()) : b.mem - a.mem);
  };
  const draw = () => {
    const v = rows();
    $('#prBody').innerHTML = `<table><thead><tr><th>Program</th><th class="num">Count</th><th class="num">CPU</th><th class="num">Memory</th><th></th></tr></thead><tbody>
      ${v.slice(0, 300).map((p, i) => `<tr><td><b>${esc(p.name)}</b><div class="muted path"><code>${esc(p.path || 'system process')}</code></div></td><td class="num">${p.pids.length}</td><td class="num">${p.cpu.toFixed(1)}%</td><td class="num">${fmt(p.mem)}</td>
      <td class="num"><button class="btn sm" data-ai="${i}">AI: explain</button> <button class="btn sm danger" data-end="${i}">End…</button></td></tr>`).join('')}</tbody></table>`;
    $$('[data-end]').forEach(b => b.onclick = async () => {
      const p = v[+b.dataset.end];
      const c = await modal({ title: `End ${esc(p.name)}?`, body: `<div>${p.pids.length > 1 ? `This ends all ${p.pids.length} running copies.` : 'This ends the program immediately.'} Unsaved work in it is lost.</div><div class="muted" style="margin-top:8px">Protected Windows processes are refused automatically.</div>`, buttons: [{ label: 'Cancel', value: false }, { label: 'End now', value: true, cls: 'danger' }] });
      if (!c.value) return;
      let ok = 0, err = null;
      for (const pid of p.pids) { try { await invoke('end_process', { pid, name: p.name }); ok++; } catch (e) { err = String(e); } }
      toast(err && !ok ? err : `${ok} ended` + (err ? ` · ${err}` : ''), !!err && !ok);
      await load(); draw();
    });
    $$('[data-ai]').forEach(b => b.onclick = () => {
      const p = v[+b.dataset.ai];
      go('ai'); setTimeout(() => sendChat(`What is the Windows process "${p.name}"${p.path ? ' running from ' + p.path : ''}? It uses ${fmt(p.mem)} of memory. Is it normal, and is it safe to end? Do not tell me to end anything unless it is clearly optional.`), 150);
    });
  };
  const ram = await invoke('health_check').then(h => `${fmt(h.ram_used)} of ${fmt(h.ram_total)} in use`).catch(() => '');
  $('#view').innerHTML = `<h2>Processes</h2><div class="sub">A live look at what is running. Memory is private working set, CPU is the share of all cores at the moment of reading. There is no "RAM booster" here: ending a program is always one explicit, confirmed action. ${ram ? '<b>' + ram + '.</b>' : ''}</div>
    <div class="row" style="margin:10px 0"><div class="chips">${[['mem', 'Memory'], ['cpu', 'CPU'], ['name', 'Name']].map(([k, l]) => `<span class="chip ${S.pSort === k ? 'on' : ''}" data-s="${k}">${l}</span>`).join('')}<span class="chip ${S.pGroup ? 'on' : ''}" id="prG">Group by program</span></div><span class="spacer"></span>
      <input type="text" id="prQ" placeholder="Search…" style="width:220px"><button class="btn" id="prRe">Refresh</button></div><div id="prBody"></div>`;
  draw();
  $('#prQ').oninput = draw;
  $('#prRe').onclick = async () => { await busy('Reading processes', load); draw(); };
  $('#prG').onclick = e => { S.pGroup = !S.pGroup; e.target.classList.toggle('on', S.pGroup); draw(); };
  $$('[data-s]').forEach(c => c.onclick = () => { S.pSort = c.dataset.s; $$('[data-s]').forEach(x => x.classList.toggle('on', x === c)); draw(); });
};

// ------------------------------------------------------------------ Browser privacy

VIEWS.browsers = async () => {
  $('#view').innerHTML = '<h2>Browser Privacy</h2><div class="sub">Looking for browser data…</div>';
  const r = await busy('Reading browser data', () => invoke('list_browser_data')).catch(e => { toast(String(e), true); return null; });
  if (!r || S.page !== 'browsers') return;
  const sel = new Set();
  const draw = () => {
    const items = r.items;
    const groups = {};
    items.forEach(i => { const k = `${i.browser}|${i.profile}|${i.category}`; (groups[k] = groups[k] || { browser: i.browser, profile: i.profile, category: i.category, files: [], size: 0 }); groups[k].files.push(i.path); groups[k].size += i.size; });
    const list = Object.values(groups);
    $('#view').innerHTML = `<h2>Browser Privacy</h2><div class="sub">Clears cookies and browsing history. Files go to <b>Quarantine</b>, so you can put them back. Passwords, bookmarks, autofill and extensions are never listed. Browser caches are cleaned on the <b>Safe Clean</b> page. The browser must be closed.</div>
      ${r.running.length ? `<div class="banner warn"><span>${esc(r.running.join(', '))} ${r.running.length > 1 ? 'are' : 'is'} open. Close ${r.running.length > 1 ? 'them' : 'it'} to clean ${r.running.length > 1 ? 'their' : 'its'} data.</span><button class="btn sm" id="brRe">Check again</button></div>` : ''}
      ${list.length ? `<table><thead><tr><th style="width:30px"></th><th>Browser</th><th>Profile</th><th>What</th><th class="num">Size</th></tr></thead><tbody>
      ${list.map((g, i) => `<tr><td><input type="checkbox" data-g="${i}" ${r.running.includes(g.browser) ? 'disabled' : ''} ${g.files.every(f => sel.has(f)) ? 'checked' : ''}></td><td><b>${esc(g.browser)}</b>${r.running.includes(g.browser) ? ' <span class="tag keep">open</span>' : ''}</td><td class="muted">${esc(g.profile)}</td>
        <td>${g.category === 'cookies' ? 'Cookies <span class="muted">(you will be signed out of websites)</span>' : 'Browsing history <span class="muted">(history, visited links, top sites, address-bar shortcuts)</span>'}</td><td class="num">${fmt(g.size)}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">No Chrome, Edge, Brave or Firefox data found.</div>'}<div id="brSel"></div>`;
    const re = $('#brRe'); if (re) re.onclick = () => VIEWS.browsers();
    $$('input[data-g]').forEach(c => c.onchange = () => {
      list[+c.dataset.g].files.forEach(f => c.checked ? sel.add(f) : sel.delete(f));
      const el = $('#brSel');
      if (!sel.size) { el.innerHTML = ''; return; }
      el.innerHTML = `<div class="sel-bar"><b>${[...$$('input[data-g]')].filter(x => x.checked).length} item(s) selected</b><span class="spacer"></span><button class="btn danger" id="brGo">Clear selected</button></div>`;
      $('#brGo').onclick = async () => {
        const ok = await modal({ title: 'Clear browser data?', body: '<div>Cookies sign you out of websites. History is removed from the address-bar suggestions. Both can be restored from <b>Quarantine & History</b> until it auto-purges.</div>', buttons: [{ label: 'Cancel', value: false }, { label: 'Clear', value: true, cls: 'danger' }] });
        if (!ok.value) return;
        const rep = await busy('Clearing', () => invoke('clean_browser', { paths: [...sel] })).catch(e => { toast(String(e), true); return null; });
        if (!rep) return;
        await alertBox(rep.ok ? 'Done' : 'Nothing was cleared', `<div><b>${rep.ok}</b> file(s) moved to Quarantine · ${fmt(rep.bytes)}</div>${failList(rep.failed)}`);
        VIEWS.browsers();
      };
    });
  };
  draw();
};

// ------------------------------------------------------------------ Schedule

VIEWS.schedule = async () => {
  $('#view').innerHTML = '<h2>Schedule</h2><div class="sub">Loading…</div>';
  const [st, choices] = await busy('Reading schedule', () => Promise.all([invoke('schedule_status'), invoke('schedule_choices')])).catch(e => { toast(String(e), true); return [null, []]; });
  await refreshBadges();
  if (S.page !== 'schedule') return;
  const s = S.settings; const ids = new Set(s.auto_clean_ids || []);
  const groups = [...new Set(choices.map(c => c.group))];
  const frNow = st ? (st.kind.includes('Daily') ? 'daily' : 'weekly') : 'weekly';
  $('#view').innerHTML = `<h2>Schedule</h2><div class="sub">Runs an automatic clean in the background through a normal Windows scheduled task (no service is installed). It only ever runs the safe targets you tick below, needs no Administrator rights, and writes every run to <b>Quarantine & History</b>.</div>
    <div class="card"><div class="row"><b>Status</b><span class="spacer"></span>${st ? `<span class="tag ${st.state === 'Ready' ? 'safe' : 'info'}">${esc(st.state)}</span>` : '<span class="tag info">not scheduled</span>'}</div>
      ${st ? `<div class="kv" style="margin-top:8px"><div>When</div><div>${frNow === 'daily' ? 'Every day' : 'Every ' + esc(dayName(st.days))} at ${esc(st.at)}</div><div>Next run</div><div>${esc(st.next ? st.next.replace('T', ' ').slice(0, 16) : '–')}</div><div>Last run</div><div>${esc(st.last ? st.last.replace('T', ' ').slice(0, 16) : 'never')}${st.last ? ' · result ' + st.result : ''}</div></div>` : ''}</div>
    <div class="card" style="margin-top:14px"><h3>Set up</h3><div class="row" style="gap:14px;margin:8px 0">
      <label>Repeat<br><select id="scF"><option value="weekly" ${frNow === 'weekly' ? 'selected' : ''}>Weekly</option><option value="daily" ${frNow === 'daily' ? 'selected' : ''}>Daily</option></select></label>
      <label id="scDl">Day<br><select id="scD">${['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'].map(d => `<option ${d === 'Sunday' ? 'selected' : ''}>${d}</option>`).join('')}</select></label>
      <label>Time<br><input type="time" id="scT" value="${st && st.at ? esc(st.at) : '12:00'}"></label></div>
      <div class="muted" style="margin:10px 0 4px">What to clean. Only safe targets that need no Administrator can run unattended. Tick a browser cache only if that browser is usually closed at that time:</div>
      ${groups.map(g => `<div style="margin:6px 0"><b>${esc(g)}</b>${choices.filter(c => c.group === g).map(c => `<label class="check"><input type="checkbox" data-id="${esc(c.id)}" ${ids.has(c.id) ? 'checked' : ''}> ${esc(c.name)} <span class="muted">· ${esc(c.tip)}</span></label>`).join('')}</div>`).join('')}
      <div class="row" style="margin-top:12px"><button class="btn primary" id="scSave">${st ? 'Update schedule' : 'Create schedule'}</button><button class="btn" id="scNow">Run a clean now</button>${st ? '<button class="btn danger" id="scDel">Remove schedule</button>' : ''}</div></div>`;
  const sync = () => $('#scDl').classList.toggle('hidden', $('#scF').value === 'daily');
  $('#scF').onchange = sync; sync();
  if (st && st.days) { const names = dayName(st.days).split(', '); if (names[0]) $('#scD').value = names[0]; }
  const saveIds = async () => {
    const picked = $$('input[data-id]').filter(c => c.checked).map(c => c.dataset.id);
    S.settings = await invoke('set_settings', { settings: { ...S.settings, auto_clean_ids: picked } });
    return picked;
  };
  $('#scSave').onclick = async () => {
    const picked = await saveIds();
    if (!picked.length) { toast('Tick at least one thing to clean.', true); return; }
    try { await invoke('schedule_set', { freq: $('#scF').value, day: $('#scD').value, time: $('#scT').value || '12:00' }); toast('Schedule saved.'); VIEWS.schedule(); } catch (e) { toast(String(e), true); }
  };
  $('#scNow').onclick = async () => {
    const picked = await saveIds();
    if (!picked.length) { toast('Tick at least one thing to clean.', true); return; }
    const r = await busy('Cleaning', () => invoke('schedule_run_now')).catch(e => { toast(String(e), true); return null; });
    if (r) { toast(`Done · ${fmt(r.freed)} freed from ${r.targets} target(s)`); refreshDrive(); }
  };
  const del = $('#scDel'); if (del) del.onclick = async () => { try { await invoke('schedule_remove'); toast('Schedule removed.'); VIEWS.schedule(); } catch (e) { toast(String(e), true); } };
};

// ------------------------------------------------------------------ Diagnostics (event log + AI)

VIEWS.diag = async () => {
  S.diagDays = S.diagDays || 7;
  $('#view').innerHTML = '<h2>Diagnostics</h2><div class="sub">Reading the Windows event log…</div>';
  const g = await busy('Reading event log', () => invoke('list_event_groups', { days: S.diagDays })).catch(e => { toast(String(e), true); return null; });
  if (!g || S.page !== 'diag') return;
  const text = (x) => `${x.provider} (event id ${x.id}, log ${x.log}, ${x.level}): ${x.count}× in ${S.diagDays} days, last ${x.last}. Example: ${x.message}`;
  $('#view').innerHTML = `<h2>Diagnostics</h2><div class="sub">Critical and error events from the System and Application logs, plus hardware-error warnings, grouped so repeats stand out. This is read-only. The AI only explains; it never changes anything. A few errors are normal on every PC, so look for ones that repeat many times.</div>
    <div class="row" style="margin:10px 0"><div class="chips">${[1, 3, 7, 14, 30].map(d => `<span class="chip ${S.diagDays === d ? 'on' : ''}" data-d="${d}">${d} day${d > 1 ? 's' : ''}</span>`).join('')}</div><span class="spacer"></span>${g.length ? '<button class="btn primary" id="dgAll">Ask AI about the top problems</button>' : ''}</div>
    ${g.length ? `<table><thead><tr><th class="num">Count</th><th>Source</th><th>Last seen</th><th>Message</th><th></th></tr></thead><tbody>${g.map((x, i) => `<tr><td class="num"><b>${x.count}</b></td><td><b>${esc(x.provider)}</b><div class="muted">id ${x.id} · ${esc(x.log)} · ${esc(x.level)}</div></td><td class="muted">${esc(x.last)}</td><td class="muted">${esc(x.message)}</td><td class="num"><button class="btn sm" data-x="${i}">Explain</button></td></tr>`).join('')}</tbody></table>` : `<div class="empty">No critical or error events in the last ${S.diagDays} day(s). 🎉</div>`}`;
  $$('[data-d]').forEach(c => c.onclick = () => { S.diagDays = +c.dataset.d; VIEWS.diag(); });
  $$('[data-x]').forEach(b => b.onclick = () => { const x = g[+b.dataset.x]; go('ai'); setTimeout(() => sendChat(`Explain this Windows event in plain language: what it means, the usual causes, whether it matters, and what I could check next. Do not suggest registry cleaners or "boost" tools.\n\n${text(x)}`), 150); });
  const all = $('#dgAll'); if (all) all.onclick = () => { go('ai'); setTimeout(() => sendChat(`These are the most frequent Windows errors on my PC in the last ${S.diagDays} days. Group them into likely root causes, tell me which ones matter and which are harmless noise, and give me a short ordered list of safe things to check. Do not suggest registry cleaners or "boost" tools.\n\n${g.slice(0, 15).map(text).join('\n')}`), 150); };
};

// ------------------------------------------------------------------ AI Assistant

const isThinkModel = n => !!(S.ollama.models.find(m => m.name === n) || {}).thinking;
const modelOpts = (cur) => {
  const names = S.ollama.models.map(m => m.name);
  if (cur && !names.includes(cur)) names.unshift(cur);
  return names.map(n => `<option value="${esc(n)}" ${n === cur ? 'selected' : ''}>${esc(n)}${isThinkModel(n) ? ' 🧠' : ''}</option>`).join('');
};
const stripThink = s => s.replace(/<think>[\s\S]*?(<\/think>|$)/g, '').trim();
function md(s) {
  let h = esc(stripThink(s)).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`([^`]+)`/g, '<code>$1</code>');
  let out = '', inList = false;
  for (const ln of h.split('\n')) {
    const m = ln.match(/^\s*(?:[-*•]|\d+\.)\s+(.*)/);
    if (m) { if (!inList) { out += '<ul>'; inList = true; } out += '<li>' + m[1] + '</li>'; }
    else { if (inList) { out += '</ul>'; inList = false; } out += ln.trim() ? ln + '<br>' : ''; }
  }
  return out + (inList ? '</ul>' : '');
}
function renderChat() {
  const log = $('#chatLog');
  if (!log) return;
  log.innerHTML = S.chat.length ? S.chat.map(m => {
      if (m.role === 'user') return `<div class="msg user">${esc(m.content)}</div>`;
      const th = m.thinking ? `<details class="think" ${m.content ? '' : 'open'}><summary>🧠 ${m.content ? 'Reasoning' : 'Thinking…'}</summary><div>${esc(m.thinking)}</div></details>` : '';
      return `<div class="msg ${m.err ? 'err' : 'assistant'}">${th}${md(m.content) || (m.thinking ? '' : '<span class="muted">…</span>')}</div>`;
    }).join('')
    : '<div class="empty">Ask anything about your disk. The assistant sees your latest scan results and runs 100% locally through Ollama.</div>';
  log.scrollTop = log.scrollHeight;
  $('#chatStop').classList.toggle('hidden', !S.aiReq);
  $('#chatSend').disabled = !!S.aiReq;
}
async function sendChat(text) {
  text = (text || '').trim();
  if (!text || S.aiReq) return;
  if (!S.ollama.running) { toast('Ollama is not running. Start it and try again.', true); return; }
  S.chat.push({ role: 'user', content: text });
  S.chat.push({ role: 'assistant', content: '' });
  const id = 'r' + Date.now();
  S.aiReq = id;
  renderChat();
  try {
    await invoke('ai_chat', { reqId: id, model: S.settings.model, messages: S.chat.slice(0, -1).filter(m => !m.err).map(m => ({ role: m.role, content: m.content })), lang: S.settings.lang });
  } catch (e) { S.aiReq = null; S.chat[S.chat.length - 1] = { role: 'assistant', content: String(e), err: true }; renderChat(); }
}
listen('ai-token', e => {
  const p = e.payload;
  if (p.id !== S.aiReq) return;
  const last = S.chat[S.chat.length - 1];
  if (last) { last.content += p.token; if (p.thinking) last.thinking = (last.thinking || '') + p.thinking; }
  if (p.done) S.aiReq = null;
  if (S.page === 'ai') renderChat();
});
listen('ai-error', e => {
  const p = e.payload;
  if (p.id !== S.aiReq) return;
  S.aiReq = null;
  S.chat[S.chat.length - 1] = { role: 'assistant', content: p.error, err: true };
  if (S.page === 'ai') renderChat();
});
function filterChips(f) {
  const c = [];
  if (f.min_mb) c.push(`≥ ${f.min_mb} MB`);
  if (f.older_than_days) c.push(`older than ${f.older_than_days} days`);
  (f.extensions || []).forEach(x => c.push(esc(x)));
  (f.name_contains || []).forEach(x => c.push(`name has “${esc(x)}”`));
  return c.map(x => `<span class="chip">${x}</span>`).join('');
}
function renderSearch() {
  const box = $('#srBody');
  if (!box || !S.search) return;
  const r = S.search.results;
  box.innerHTML = `<div class="row" style="margin-bottom:8px"><span class="tag ai">AI read your request as</span><div class="chips">${filterChips(S.search.filter)}</div></div>
    ${S.search.filter.summary ? `<div class="muted" style="margin-bottom:8px">${esc(S.search.filter.summary)}</div>` : ''}
    ${r.length ? fileTable(r, S.searchSel, 'sr') + '<div id="srSel"></div>' : '<div class="empty">No matching files.</div>'}`;
  if (r.length) bindFileTable(box, r, S.searchSel, 'sr', $('#srSel'), gone => {
    S.search.results = S.search.results.filter(f => !gone.includes(f.path)); S.searchSel = new Set(); renderSearch();
  });
}
VIEWS.ai = async () => {
  await refreshBadges();
  const prompts = ['What should I clean first?', 'Summarize my scan results', 'How can I free 50 GB without touching my projects?', 'Which big folders could move to another drive?'];
  $('#view').innerHTML = `<h2>AI Assistant</h2><div class="sub">Local Ollama only. It explains and suggests. It cannot delete anything; every removal needs your click and passes the safety rules.</div>
    ${S.ollama.running ? '' : '<div class="banner warn"><span>Ollama is not reachable. Start Ollama and reopen this tab.</span></div>'}
    <div class="row" style="margin-bottom:12px"><label class="muted">Model <select id="aiModel">${modelOpts(S.settings.model)}</select></label>
      <label class="muted ${isThinkModel(S.settings.model) ? '' : 'hidden'}" id="aiThinkWrap" title="Lets the model reason before answering. Slower, sometimes more careful."><input type="checkbox" id="aiThink" ${(S.settings.think || {})[S.settings.model] ? 'checked' : ''}> 🧠 Thinking <span class="muted">(slower)</span></label>
      <label class="muted">Reply language <select id="aiLang"><option value="bn" ${S.settings.lang === 'bn' ? 'selected' : ''}>বাংলা</option><option value="en" ${S.settings.lang === 'en' ? 'selected' : ''}>English</option></select></label>
      <span class="spacer"></span><button class="btn sm" id="chatClear">Clear chat</button></div>
    <div class="grid g2">
      <div class="card"><h3>Ask about your disk</h3>
        <div class="chips" style="margin-bottom:10px">${prompts.map(p => `<span class="chip" data-q="${esc(p)}">${esc(p)}</span>`).join('')}</div>
        <div class="chat"><div id="chatLog"></div>
          <div class="row" style="margin-top:8px"><input type="text" id="chatIn" style="flex:1" placeholder="Type a question…"><button class="btn primary" id="chatSend">Send</button><button class="btn hidden" id="chatStop">Stop</button></div></div></div>
      <div class="card"><h3>Smart file search</h3>
        <div class="muted" style="margin-bottom:8px">Describe what to find, in English or Bengali. The AI turns it into a filter; the search itself is exact.</div>
        <input type="text" id="srQ" style="width:100%" placeholder="e.g. videos bigger than 1 GB older than 6 months">
        <div class="row" style="margin:8px 0"><input type="text" id="srRoot" style="flex:1" placeholder="Search in (default: your user folder)"><button class="btn primary" id="srGo">Search</button></div>
        <div id="srBody"></div></div>
    </div>`;
  $('#aiModel').onchange = async e => {
    S.settings.model = e.target.value;
    S.settings = await invoke('set_settings', { settings: S.settings });
    $('#aiThinkWrap').classList.toggle('hidden', !isThinkModel(S.settings.model));
    $('#aiThink').checked = !!(S.settings.think || {})[S.settings.model];
  };
  $('#aiThink').onchange = async e => {
    S.settings.think = { ...(S.settings.think || {}), [S.settings.model]: e.target.checked };
    S.settings = await invoke('set_settings', { settings: S.settings });
    toast(e.target.checked ? 'Thinking on for ' + S.settings.model + ' (answers are slower)' : 'Thinking off for ' + S.settings.model);
  };
  $('#aiLang').onchange = async e => { S.settings.lang = e.target.value; await invoke('set_settings', { settings: S.settings }); };
  $('#chatClear').onclick = () => { if (S.aiReq) invoke('ai_cancel', { reqId: S.aiReq }); S.aiReq = null; S.chat = []; renderChat(); };
  $$('[data-q]').forEach(c => c.onclick = () => sendChat(c.dataset.q));
  const send = () => { const i = $('#chatIn'); const t = i.value; i.value = ''; sendChat(t); };
  $('#chatSend').onclick = send;
  $('#chatIn').onkeydown = e => { if (e.key === 'Enter') send(); };
  $('#chatStop').onclick = () => { if (S.aiReq) invoke('ai_cancel', { reqId: S.aiReq }); S.aiReq = null; renderChat(); };
  $('#srGo').onclick = async () => {
    const q = $('#srQ').value.trim();
    if (!q) return;
    if (!S.ollama.running) { toast('Ollama is not running.', true); return; }
    const r = await busy('Asking ' + S.settings.model + ' and searching', () => invoke('ai_search', { query: q, model: S.settings.model, root: $('#srRoot').value })).catch(e => { toast(String(e), true); return null; });
    if (!r) return;
    S.search = r; S.searchSel = new Set(); renderSearch();
  };
  $('#srQ').onkeydown = e => { if (e.key === 'Enter') $('#srGo').click(); };
  renderChat(); renderSearch();
};

// ------------------------------------------------------------------ Quarantine & History

const ACTION_LABEL = { 'safe-clean': 'Safe Clean', quarantine: 'Quarantined', delete: 'Deleted', restore: 'Restored', purge: 'Purged quarantine', 'auto-purge': 'Auto-purged quarantine' };
VIEWS.quarantine = async () => {
  const [q, hist] = await Promise.all([invoke('list_quarantine'), invoke('get_history')]);
  const tot = q.reduce((s, i) => s + i.size, 0);
  const days = S.settings.quarantine_days;
  $('#view').innerHTML = `<h2>Quarantine & History</h2><div class="sub">Removed items wait here so you can undo. Disk space returns when they are purged${days ? ` (automatically after ${days} days)` : ''}.</div>
    <div class="row" style="margin-bottom:10px"><b>${q.length} item(s) · ${fmt(tot)}</b><span class="spacer"></span>
      <button class="btn" id="qRestore" disabled>Restore selected</button><button class="btn" id="qPurge" disabled>Purge selected</button>
      <button class="btn danger" id="qPurgeAll" ${q.length ? '' : 'disabled'}>Purge all (free ${fmt(tot)})</button></div>
    ${q.length ? `<table><thead><tr><th></th><th>Original location</th><th class="num">Size</th><th class="num">Quarantined</th></tr></thead><tbody>
      ${q.map((i, n) => `<tr><td style="width:30px"><input type="checkbox" data-q="${n}"></td><td><code>${esc(i.orig)}</code></td><td class="num">${fmt(i.size)}</td><td class="num muted">${fdt(i.ts)}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">Quarantine is empty.</div>'}
    <h3 style="margin-top:26px">History</h3>
    ${hist.length ? `<table><thead><tr><th>When</th><th>Action</th><th>Detail</th><th class="num">Size</th></tr></thead><tbody>
      ${hist.slice(0, 80).map(h => `<tr><td class="muted">${fdt(h.ts)}</td><td>${esc(ACTION_LABEL[h.action] || h.action)}</td><td class="muted">${esc(h.detail)}</td><td class="num">${fmt(h.bytes)}</td></tr>`).join('')}</tbody></table>` : '<div class="empty">No actions yet.</div>'}`;
  const sel = () => $$('input[data-q]').filter(i => i.checked).map(i => q[+i.dataset.q].id);
  const upd = () => { const n = sel().length; $('#qRestore').disabled = !n; $('#qPurge').disabled = !n; };
  $$('input[data-q]').forEach(i => i.onchange = upd);
  $('#qRestore').onclick = async () => {
    const rep = await invoke('restore_quarantine', { ids: sel() });
    await alertBox('Restore', `<b>${rep.ok}</b> item(s) restored.${failList(rep.failed)}`); await refreshDrive(); VIEWS.quarantine();
  };
  const purge = async (ids, all) => {
    const r = await modal({ title: 'Purge permanently?', body: 'Purged items are deleted for good and the disk space is released.', buttons: [{ label: 'Cancel', value: false }, { label: 'Purge', value: true, cls: 'danger' }] });
    if (!r.value) return;
    const rep = await busy('Purging', () => invoke('purge_quarantine', { ids, all }));
    await alertBox('Purge', `<b>${rep.ok}</b> item(s) purged · ${fmt(rep.bytes)} released.${failList(rep.failed)}`); await refreshDrive(); VIEWS.quarantine();
  };
  $('#qPurge').onclick = () => purge(sel(), false);
  $('#qPurgeAll').onclick = () => purge([], true);
};

// ------------------------------------------------------------------ Settings

VIEWS.settings = async () => {
  await refreshBadges();
  const s = S.settings;
  const thinkers = S.ollama.models.filter(m => m.thinking);
  $('#view').innerHTML = `<h2>Settings</h2><div class="sub">Stored in %LOCALAPPDATA%\\CDriveStudio</div>
    <div class="card" style="max-width:760px"><div class="grid" style="gap:14px">
      <label>Ollama address<br><input type="text" id="stHost" style="width:100%" value="${esc(s.ollama_host)}"></label>
      <div class="row"><label>Default model<br><select id="stModel">${modelOpts(s.model)}</select></label>
        <label>AI reply language<br><select id="stLang"><option value="bn" ${s.lang === 'bn' ? 'selected' : ''}>বাংলা</option><option value="en" ${s.lang === 'en' ? 'selected' : ''}>English</option></select></label>
        <label>Quarantine auto-purge (days, 0 = never)<br><input type="number" id="stDays" min="0" value="${s.quarantine_days}" style="width:120px"></label></div>
      <div><div style="margin-bottom:6px">Thinking (reasoning) models <span class="muted">· detected from Ollama. Off is faster; turn on only where you want deeper answers.</span></div>
        ${thinkers.length ? thinkers.map(m => `<label class="check"><input type="checkbox" data-th="${esc(m.name)}" ${(s.think || {})[m.name] ? 'checked' : ''}> 🧠 ${esc(m.name)}</label>`).join('') : '<span class="muted">None of the installed models supports thinking.</span>'}</div>
      <label class="check"><input type="checkbox" id="stTray" ${s.close_to_tray ? 'checked' : ''}> Keep running in the system tray when I close the window <span class="muted">· open or quit from the tray icon</span></label>
      <label>Protected paths (one per line). Nothing under these can be removed through the app.<br><textarea id="stProt" rows="5" style="width:100%">${esc(s.protected.join('\n'))}</textarea></label>
      <div class="row"><button class="btn primary" id="stSave">Save</button><button class="btn" id="stTest">Test Ollama connection</button></div>
      <div class="muted">Built-in protections that cannot be turned off: Windows, Program Files, ProgramData\\Microsoft, system files, drive roots, and your core user folders.</div>
    </div></div>`;
  $('#stSave').onclick = async () => {
    const th = {}; $$('input[data-th]').forEach(c => th[c.dataset.th] = c.checked);
    const ns = { ...S.settings, think: { ...(S.settings.think || {}), ...th }, ollama_host: $('#stHost').value.trim(), model: $('#stModel').value, lang: $('#stLang').value, quarantine_days: Math.max(0, +$('#stDays').value || 0), close_to_tray: $('#stTray').checked, protected: $('#stProt').value.split('\n').map(x => x.trim()).filter(Boolean) };
    S.settings = await invoke('set_settings', { settings: ns });
    toast('Settings saved'); refreshBadges();
  };
  $('#stTest').onclick = async () => {
    await $('#stSave').onclick();
    await refreshBadges();
    toast(S.ollama.running ? `Ollama OK · ${S.ollama.models.length} model(s)` : 'Cannot reach Ollama', !S.ollama.running);
  };
};

// ------------------------------------------------------------------ boot

(async function init() {
  try {
    buildNav();
    S.settings = await invoke('get_settings');
    await Promise.all([refreshDrive(), refreshBadges()]);
    go('dashboard');
    setInterval(refreshBadges, 30000);
  } catch (e) {
    $('#view').innerHTML = `<div class="empty">Startup error: ${esc(e)}</div>`;
  }
})();
