/* ── AirNote Enterprise Admin ─────────────────────────────────────────────
 *  Veselity-style dashboard with Cursor aesthetics. Light + dark mode.
 *  Vanilla JS SPA — no build step.
 * ──────────────────────────────────────────────────────────────────────── */
'use strict';

const API = '';
const THEME_KEY = 'airnote:theme';
const LEGACY_THEME_KEY = 'said:theme';
const AUTH_TOKEN_KEY = 'airnote:admin:token';
const LEGACY_AUTH_TOKEN_KEY = 'said:admin:token';

// ── Theme ───────────────────────────────────────────────────────────────

function getTheme() {
  const theme = localStorage.getItem(THEME_KEY);
  if (theme) return theme;
  const legacyTheme = localStorage.getItem(LEGACY_THEME_KEY);
  if (legacyTheme) {
    localStorage.setItem(THEME_KEY, legacyTheme);
    localStorage.removeItem(LEGACY_THEME_KEY);
    return legacyTheme;
  }
  return 'light';
}
function setTheme(t) {
  localStorage.setItem(THEME_KEY, t);
  localStorage.removeItem(LEGACY_THEME_KEY);
  document.documentElement.setAttribute('data-theme', t);
}
function toggleTheme() { setTheme(getTheme() === 'dark' ? 'light' : 'dark'); updateThemeIcon(); }
function initTheme() { setTheme(getTheme()); }
function updateThemeIcon() {
  const el = document.getElementById('theme-toggle');
  if (el) el.innerHTML = getTheme() === 'dark' ? I.sun : I.moon;
}

// ── Auth ────────────────────────────────────────────────────────────────

function getToken() {
  const token = localStorage.getItem(AUTH_TOKEN_KEY);
  if (token) return token;
  const legacyToken = localStorage.getItem(LEGACY_AUTH_TOKEN_KEY);
  if (legacyToken) {
    localStorage.setItem(AUTH_TOKEN_KEY, legacyToken);
    localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY);
  }
  return legacyToken;
}
function setToken(t) {
  localStorage.setItem(AUTH_TOKEN_KEY, t);
  localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY);
}
function clearToken() {
  localStorage.removeItem(AUTH_TOKEN_KEY);
  localStorage.removeItem(LEGACY_AUTH_TOKEN_KEY);
}
function logout() { const t = getToken(); if (t) api('/v1/auth/logout', { method: 'POST' }).catch(() => {}); clearToken(); cachedUser = null; cachedOrg = null; render(); }

// ── API ─────────────────────────────────────────────────────────────────

async function api(path, opts = {}) {
  const token = getToken(), headers = { ...opts.headers };
  if (opts.body && typeof opts.body === 'string') headers['Content-Type'] = 'application/json';
  if (token) headers['Authorization'] = 'Bearer ' + token;
  return fetch(API + path, { ...opts, headers });
}
async function apiJson(path, opts = {}) {
  const res = await api(path, opts);
  if (res.status === 204) return null;
  const text = await res.text();
  let data; try { data = JSON.parse(text); } catch { throw new Error(text || `Request failed (${res.status})`); }
  if (!res.ok) throw new Error(data.error || `Request failed (${res.status})`);
  return data;
}

// ── State ───────────────────────────────────────────────────────────────

let cachedUser = null, cachedOrg = null;
async function fetchUser() { if (cachedUser) return cachedUser; try { cachedUser = await apiJson('/v1/auth/me'); return cachedUser; } catch { clearToken(); render(); return null; } }
async function fetchOrg() { if (cachedOrg) return cachedOrg; try { cachedOrg = await apiJson('/v1/orgs/me'); return cachedOrg; } catch { return null; } }

// ── Helpers ─────────────────────────────────────────────────────────────

function esc(s) { if (s == null) return ''; const d = document.createElement('div'); d.textContent = String(s); return d.innerHTML; }
function fmtDate(s) { if (!s) return '--'; return new Date(s).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' }); }
function fmtTime(s) { if (!s) return ''; return new Date(s).toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' }); }
function timeAgo(s) { if (!s) return '--'; const m = Math.floor((Date.now() - new Date(s)) / 60000); if (m < 1) return 'Just now'; if (m < 60) return m + 'm ago'; const h = Math.floor(m / 60); if (h < 24) return h + 'h ago'; const d = Math.floor(h / 24); return d < 7 ? d + 'd ago' : fmtDate(s); }
function dur(a, b) { if (!a || !b) return '--'; const m = Math.round((new Date(b) - new Date(a)) / 60000); return m < 60 ? m + ' min' : Math.floor(m / 60) + 'h ' + (m % 60) + 'm'; }

const AV_COLORS = ['#6366f1','#8b5cf6','#ec4899','#f43f5e','#f97316','#eab308','#22c55e','#14b8a6','#06b6d4','#3b82f6'];
function nameHash(n) { let h = 0; for (let i = 0; i < (n||'').length; i++) h = n.charCodeAt(i) + ((h << 5) - h); return Math.abs(h); }
function avColor(n) { return AV_COLORS[nameHash(n || '?') % AV_COLORS.length]; }
function avInit(n) { if (!n) return '?'; const p = n.trim().split(/\s+/); return p.length === 1 ? p[0][0].toUpperCase() : (p[0][0] + p[p.length-1][0]).toUpperCase(); }
function av(n, cls) { return `<div class="${cls || 'av'}" style="background:${avColor(n)}">${esc(avInit(n))}</div>`; }

function statusPill(s) {
  const m = { scheduled:'pill-blue', live:'pill-red', ended:'pill-green' };
  return `<span class="pill ${m[s]||'pill-neutral'}"><span class="pill-dot"></span>${esc(s?s[0].toUpperCase()+s.slice(1):'--')}</span>`;
}
function rolePill(r) { if (!r) return ''; const l = r.toLowerCase(); if (l==='admin'||l==='company_admin') return '<span class="pill pill-blue">Admin</span>'; if (l==='manager') return '<span class="pill pill-amber">Manager</span>'; return '<span class="pill pill-neutral">Member</span>'; }

// ── Icons ───────────────────────────────────────────────────────────────

const I = {
  home:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></svg>',
  clock:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>',
  users:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/></svg>',
  chat:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>',
  analytics:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg>',
  integrations:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8m-4-4v4"/></svg>',
  settings:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"/><path d="M12 1v2m0 18v2M4.22 4.22l1.42 1.42m12.73 12.73l1.42 1.42M1 12h2m18 0h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>',
  members:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75"/></svg>',
  feedback:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/></svg>',
  logout:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" y1="12" x2="9" y2="12"/></svg>',
  plus:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M12 5v14m-7-7h14"/></svg>',
  back:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M19 12H5m7-7l-7 7 7 7"/></svg>',
  search:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/></svg>',
  bell:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.73 21a2 2 0 0 1-3.46 0"/></svg>',
  brand:'<svg viewBox="0 0 24 24" fill="none"><rect x="3" y="8.5" width="3" height="7" rx="1.5" fill="currentColor"/><rect x="8" y="4.5" width="3" height="15" rx="1.5" fill="currentColor"/><rect x="13" y="2.5" width="3" height="19" rx="1.5" fill="currentColor"/><rect x="18" y="6.5" width="3" height="11" rx="1.5" fill="currentColor"/></svg>',
  arrow:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 12h14m-7-7l7 7-7 7"/></svg>',
  sparkle:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M12 2l2.09 6.26L20 10.27l-4.91 3.82L16.18 22 12 17.77 7.82 22l1.09-7.91L4 10.27l5.91-1.01z"/></svg>',
  mic:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="9" y="1" width="6" height="12" rx="3"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" y1="19" x2="12" y2="23"/></svg>',
  check:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><path d="M20 6L9 17l-5-5"/></svg>',
  doc:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/></svg>',
  calendar:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/></svg>',
  filter:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 3H2l8 9.46V19l4 2v-8.54L22 3z"/></svg>',
  info:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 16v-4m0-4h.01"/></svg>',
  sun:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="5"/><path d="M12 1v2m0 18v2M4.22 4.22l1.42 1.42m12.73 12.73l1.42 1.42M1 12h2m18 0h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>',
  moon:'<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>',
};
function icon(n, s) { const z = s || 16; return `<span style="display:inline-flex;width:${z}px;height:${z}px">${I[n]||''}</span>`; }

// ── Router ──────────────────────────────────────────────────────────────

function parseRoute() {
  const p = window.location.pathname.replace(/^\/admin\/?/, '').split('/').filter(Boolean);
  if (!p.length) return { page: 'dashboard' };
  if (p[0]==='meetings'&&p[1]==='new') return { page: 'meeting-new' };
  if (p[0]==='meetings'&&p[1]) return { page: 'meeting-detail', id: p[1] };
  if (p[0]==='meetings') return { page: 'meetings' };
  if (p[0]==='team') return { page: 'team' };
  if (p[0]==='settings') return { page: 'settings' };
  if (p[0]==='login') return { page: 'login' };
  return { page: 'dashboard' };
}
function navigate(path) { history.pushState({}, '', path); render(); }

// ── Render ───────────────────────────────────────────────────────────────

function render() {
  initTheme();
  const app = document.getElementById('app'), token = getToken();
  if (!token) { app.innerHTML = renderLogin(); bindLogin(); return; }
  const route = parseRoute();
  if (route.page === 'login') { navigate('/admin/'); return; }
  app.innerHTML = renderShell(route);
  bindShell();
  loadPage(route);
}
async function loadPage(route) {
  const el = document.getElementById('page-content');
  if (!el) return;
  try {
    switch (route.page) {
      case 'dashboard': el.innerHTML = await renderDashboard(); break;
      case 'meetings': el.innerHTML = await renderMeetings(); bindMeetings(); break;
      case 'meeting-detail': el.innerHTML = await renderMeetingDetail(route.id); bindMeetingDetail(route.id); break;
      case 'meeting-new': el.innerHTML = await renderMeetingNew(); bindMeetingNew(); break;
      case 'team': el.innerHTML = await renderTeam(); break;
      case 'settings': el.innerHTML = await renderSettings(); bindSettings(); break;
      default: el.innerHTML = await renderDashboard();
    }
  } catch (err) { el.innerHTML = renderError('Failed to load page', err.message); }
}
window.addEventListener('popstate', () => render());
window.addEventListener('DOMContentLoaded', render);
document.addEventListener('click', e => { const a = e.target.closest('a[href^="/admin"]'); if (a && !e.ctrlKey && !e.metaKey) { e.preventDefault(); navigate(a.getAttribute('href')); } });

// ── Shell ───────────────────────────────────────────────────────────────

function renderShell(route) {
  const pg = route.page;
  const nav = (href, ic, label, match) => {
    const active = typeof match === 'function' ? match(pg) : pg === match;
    return `<a href="${href}" class="${active?'active':''}">${icon(ic,15)} ${label}</a>`;
  };
  return `
  <div class="layout">
    <aside class="sidebar">
      <div class="s-brand">
        <div class="s-brand-icon">${icon('brand',14)}</div>
        <div><div class="s-brand-text">AirNote</div><div class="s-brand-sub">Enterprise</div></div>
      </div>
      <div class="s-section">Main menu</div>
      <nav class="s-nav">
        ${nav('/admin/','home','Dashboard','dashboard')}
        ${nav('/admin/meetings','clock','Meetings',p=>p==='meetings'||p==='meeting-detail'||p==='meeting-new')}
        ${nav('/admin/team','users','Team','team')}
      </nav>
      <div class="s-section">Other</div>
      <nav class="s-nav">
        ${nav('/admin/settings','settings','Settings','settings')}
      </nav>
      <div class="s-spacer"></div>
      <div class="s-bottom">
        <button onclick="logout()">${icon('logout',14)} Sign out</button>
      </div>
      <div class="s-user" id="sidebar-user">${av('...',  'av')} <div><div class="s-user-name">Loading...</div><div class="s-user-sub">--</div></div></div>
    </aside>
    <div class="content-wrap">
      <div class="mat">
        <div class="topbar">
          <div class="topbar-search">${icon('search',14)} Search... <kbd>/</kbd></div>
          <div class="topbar-right">
            <div class="topbar-icon" id="theme-toggle" onclick="toggleTheme()" title="Toggle theme">${getTheme()==='dark'?I.sun:I.moon}</div>
            <div class="topbar-icon">${I.bell}</div>
            <button class="btn btn-primary" onclick="navigate('/admin/meetings/new')">${icon('plus',13)} New Meeting</button>
            <div class="av-topbar" id="topbar-av" style="background:#6366f1">...</div>
          </div>
        </div>
        <div class="page" id="page-content"><div class="loading-state"><div class="spinner"></div> Loading...</div></div>
      </div>
    </div>
  </div>`;
}

function bindShell() {
  fetchUser().then(u => {
    if (!u) return;
    const email = u.account ? u.account.email : '';
    const name = email.split('@')[0] || 'Admin';
    const display = name.charAt(0).toUpperCase() + name.slice(1);
    const tier = u.license ? u.license.tier : 'free';
    const su = document.getElementById('sidebar-user');
    if (su) su.innerHTML = `${av(display,'av')} <div><div class="s-user-name">${esc(display)}</div><div class="s-user-sub">${esc(email)}</div></div>`;
    const ta = document.getElementById('topbar-av');
    if (ta) { ta.style.background = avColor(display); ta.textContent = avInit(display); }
  });
}

// ── Login ───────────────────────────────────────────────────────────────

function renderLogin() {
  return `<div class="login-page"><div class="login-card">
    <div class="login-brand"><span style="display:inline-flex;width:22px;height:22px">${I.brand}</span><span class="login-brand-text">AirNote Enterprise</span></div>
    <h1>Welcome back</h1><p class="login-sub">Sign in to your admin dashboard</p>
    <div id="login-error" style="display:none"></div>
    <form id="login-form">
      <div class="form-group"><label class="form-label">Email</label><input type="email" class="form-input" id="login-email" placeholder="you@company.com" required autocomplete="email"/></div>
      <div class="form-group"><label class="form-label">Password</label><input type="password" class="form-input" id="login-password" placeholder="Enter your password" required autocomplete="current-password"/></div>
      <button type="submit" class="btn btn-primary" id="login-btn" style="width:100%;margin-top:4px">Sign In</button>
    </form>
    <div class="login-toggle"><span id="login-mode-text">Don't have an account?</span> <a id="login-mode-toggle">Sign Up</a></div>
  </div></div>`;
}
function bindLogin() {
  let signup = false;
  const form = document.getElementById('login-form'), btn = document.getElementById('login-btn'), tt = document.getElementById('login-mode-text'), tl = document.getElementById('login-mode-toggle'), err = document.getElementById('login-error');
  tl.addEventListener('click', () => { signup = !signup; btn.textContent = signup ? 'Create Account' : 'Sign In'; tt.textContent = signup ? 'Already have an account?' : "Don't have an account?"; tl.textContent = signup ? 'Sign In' : 'Sign Up'; err.style.display = 'none'; });
  form.addEventListener('submit', async e => {
    e.preventDefault(); const email = document.getElementById('login-email').value.trim(), pass = document.getElementById('login-password').value;
    if (!email || pass.length < 8) { err.className = 'login-error'; err.textContent = 'Email required, password min 8 characters.'; err.style.display = 'block'; return; }
    btn.disabled = true; btn.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div>'; err.style.display = 'none';
    try { const res = await api(signup ? '/v1/auth/signup' : '/v1/auth/login', { method: 'POST', body: JSON.stringify({ email, password: pass }) }); const data = await res.json(); if (!res.ok) throw new Error(data.error || 'Authentication failed'); setToken(data.token); cachedUser = null; cachedOrg = null; navigate('/admin/'); }
    catch (e) { err.className = 'login-error'; err.textContent = e.message; err.style.display = 'block'; btn.disabled = false; btn.textContent = signup ? 'Create Account' : 'Sign In'; }
  });
}

// ── Dashboard ───────────────────────────────────────────────────────────

async function renderDashboard() {
  const [orgData, mtgData] = await Promise.all([fetchOrg(), apiJson('/v1/meetings').catch(() => ({ meetings: [] }))]);
  const meetings = mtgData.meetings || [];
  const orgName = orgData && orgData.org ? orgData.org.name : 'Your Organization';
  const total = meetings.length, live = meetings.filter(m => m.status === 'live').length, ended = meetings.filter(m => m.status === 'ended').length;
  const recent = meetings.slice(0, 5);

  return `
    <div style="display:flex;justify-content:space-between;align-items:flex-start">
      <div><div class="page-title">Dashboard</div><div class="page-sub">Track your meetings and team performance &middot; ${esc(orgName)}</div></div>
      ${live > 0 ? `<div class="badge-live"><div class="badge-live-dot"></div> ${live} Live</div>` : ''}
    </div>

    <!-- Top metrics -->
    <div class="top-metrics">
      <div class="mc">
        <div class="mc-head"><span class="mc-label">Meeting overview</span><span class="mc-tab">This month</span></div>
        <div class="mc-val">${total}<span class="mc-val-sm">Total meetings</span></div>
        <div class="mc-sub">${ended} completed &middot; ${live} live</div>
        <div class="mc-pills"><button class="mc-pill on">All</button><button class="mc-pill off">Completed</button></div>
      </div>
      <div class="mc">
        <div class="mc-head"><span class="mc-label">AI tasks generated</span></div>
        <div class="mc-val">--</div>
        <div class="mc-sub">Connect OpenAI in Settings</div>
        <div class="mc-bars">${[35,50,40,65,55,75,60,90,100].map((h,i) => `<div class="mc-bar${i>=7?' hi':''}" style="height:${h}%"></div>`).join('')}</div>
        <div class="mc-link">See Details ${icon('arrow',12)}</div>
      </div>
      <div class="mc">
        <div class="mc-head"><span class="mc-label">Words transcribed</span></div>
        <div class="mc-val">--</div>
        <div class="mc-sub">Across all meetings</div>
        <div class="mc-line"><svg viewBox="0 0 200 50" preserveAspectRatio="none" style="width:100%;height:100%"><polyline points="0,40 25,35 50,28 75,32 100,22 125,18 150,25 175,12 200,8" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><circle cx="200" cy="8" r="3" fill="var(--accent)"/></svg></div>
        <div class="mc-link">See Details ${icon('arrow',12)}</div>
      </div>
    </div>

    <!-- Analytics + Gauge -->
    <div class="mid-row">
      <div class="panel">
        <div class="panel-head">
          <div class="panel-title">Analytics ${icon('info',14)}</div>
          <div class="panel-tabs">
            <button class="panel-tab on">This year</button>
            <button class="panel-filter">${icon('filter',12)} Filters</button>
          </div>
        </div>
        <div class="ap-stats">
          <div><div class="ap-stat-val">${total}<span class="ap-badge up">+12%</span></div><div class="ap-stat-label">Total meetings</div></div>
          <div><div class="ap-stat-val">${ended}<span class="ap-badge up">completed</span></div><div class="ap-stat-label">With AI summary</div></div>
        </div>
        <div class="area-chart">
          <svg viewBox="0 0 600 180" preserveAspectRatio="none" style="width:100%;height:180px;display:block">
            <defs>
              <pattern id="hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="6" stroke="var(--accent)" stroke-width="1.2" stroke-opacity="var(--hatch-opacity)"/></pattern>
              <linearGradient id="aFade" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="var(--accent)" stop-opacity="var(--grad-top)"/><stop offset="100%" stop-color="var(--accent)" stop-opacity="var(--grad-bot)"/></linearGradient>
            </defs>
            <line x1="0" y1="0" x2="600" y2="0" stroke="var(--border-light)" stroke-width="1"/><line x1="0" y1="60" x2="600" y2="60" stroke="var(--border-light)" stroke-width="1"/><line x1="0" y1="120" x2="600" y2="120" stroke="var(--border-light)" stroke-width="1"/><line x1="0" y1="180" x2="600" y2="180" stroke="var(--border-light)" stroke-width="1"/>
            <polygon points="0,180 0,140 50,130 100,120 150,115 200,100 250,110 300,80 350,90 400,60 450,70 500,40 550,50 600,30 600,180" fill="url(#hatch)"/>
            <polygon points="0,180 0,140 50,130 100,120 150,115 200,100 250,110 300,80 350,90 400,60 450,70 500,40 550,50 600,30 600,180" fill="url(#aFade)"/>
            <polyline points="0,140 50,130 100,120 150,115 200,100 250,110 300,80 350,90 400,60 450,70 500,40 550,50 600,30" fill="none" stroke="var(--accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"/>
            <circle cx="400" cy="60" r="5" fill="var(--surface)" stroke="var(--accent)" stroke-width="2.5"/>
            <circle cx="400" cy="60" r="10" fill="none" stroke="var(--accent)" stroke-width="1" opacity="0.2"/>
          </svg>
          <div class="chart-tip" style="left:66.5%;top:18%">${total} meetings</div>
        </div>
        <div class="area-x"><span>JAN</span><span>FEB</span><span>MAR</span><span>APR</span><span class="hl">MAY</span><span>JUN</span><span>JUL</span><span>AUG</span></div>
      </div>

      <div class="panel">
        <div class="panel-head"><div class="panel-title">AI Performance</div><div class="mc-link">See Details ${icon('arrow',12)}</div></div>
        <div class="gauge-wrap">
          <svg class="gauge-svg" viewBox="0 0 200 110"><path d="M 20 100 A 80 80 0 0 1 180 100" fill="none" stroke="var(--border)" stroke-width="14" stroke-linecap="round"/><path d="M 20 100 A 80 80 0 0 1 180 100" fill="none" stroke="var(--accent)" stroke-width="14" stroke-linecap="round" stroke-dasharray="251.2" stroke-dashoffset="80"/><circle cx="100" cy="100" r="4" fill="var(--accent)"/></svg>
          <div class="gauge-val">${ended > 0 ? Math.round(ended / total * 100) + '%' : '--'}</div>
          <div class="gauge-sub">Meetings with AI output</div>
        </div>
        <div class="gauge-legend">
          <div class="gl-row"><div class="gl-dot" style="background:var(--accent)"></div> Tasks per meeting <div class="gl-right"><button class="panel-tab on">Week</button><button class="panel-tab off">Month</button></div></div>
          <div class="gl-row"><div class="gl-dot" style="background:var(--fg-5)"></div> Average across org</div>
        </div>
      </div>
    </div>

    <!-- Recent meetings -->
    <div class="panel">
      <div class="panel-head"><div class="panel-title">Recent Meetings</div><a href="/admin/meetings" class="mc-link">View all ${icon('arrow',12)}</a></div>
      ${recent.length === 0 ? '<div class="empty-state" style="padding:30px"><h3>No meetings yet</h3><p>Create your first meeting to get started.</p></div>'
      : `<table><thead><tr><th>Meeting</th><th>Date</th><th>Duration</th><th>Status</th></tr></thead><tbody>${recent.map(m => `
        <tr style="cursor:pointer" onclick="navigate('/admin/meetings/${m.id}')">
          <td>${esc(m.title)}</td><td>${fmtDate(m.created_at)}</td><td>${dur(m.started_at, m.ended_at)}</td><td>${statusPill(m.status)}</td>
        </tr>`).join('')}</tbody></table>`}
    </div>`;
}

// ── Meetings list ───────────────────────────────────────────────────────

let mtgFilter = null;
async function renderMeetings() {
  const data = await apiJson('/v1/meetings').catch(() => ({ meetings: [] }));
  const meetings = data.meetings || [];
  return `
    <div style="display:flex;justify-content:space-between;align-items:flex-start"><div><div class="page-title">Meetings</div><div class="page-sub">${meetings.length} total</div></div></div>
    <div class="filter-bar" id="meeting-filters">
      <button class="filter-btn ${mtgFilter===null?'active':''}" data-status="">All</button>
      <button class="filter-btn ${mtgFilter==='scheduled'?'active':''}" data-status="scheduled">Scheduled</button>
      <button class="filter-btn ${mtgFilter==='live'?'active':''}" data-status="live">Live</button>
      <button class="filter-btn ${mtgFilter==='ended'?'active':''}" data-status="ended">Ended</button>
    </div>
    <div class="panel" id="meetings-table">${mtgTable(meetings)}</div>`;
}
function mtgTable(meetings) {
  const f = mtgFilter ? meetings.filter(m => m.status === mtgFilter) : meetings;
  if (!f.length) return '<div class="empty-state"><h3>No meetings found</h3></div>';
  return `<table><thead><tr><th>Title</th><th>Date</th><th>Duration</th><th>Status</th><th>Created</th></tr></thead><tbody>${f.map(m => `
    <tr style="cursor:pointer" onclick="navigate('/admin/meetings/${m.id}')"><td>${esc(m.title)}</td><td>${fmtDate(m.created_at)}</td><td>${dur(m.started_at, m.ended_at)}</td><td>${statusPill(m.status)}</td><td style="color:var(--fg-4)">${timeAgo(m.created_at)}</td></tr>`).join('')}</tbody></table>`;
}
function bindMeetings() {
  const bar = document.getElementById('meeting-filters'); if (!bar) return;
  bar.addEventListener('click', async e => { const b = e.target.closest('.filter-btn'); if (!b) return; mtgFilter = b.dataset.status || null; bar.querySelectorAll('.filter-btn').forEach(x => x.classList.remove('active')); b.classList.add('active');
    try { const d = await apiJson('/v1/meetings' + (mtgFilter ? '?status=' + mtgFilter : '')); document.getElementById('meetings-table').innerHTML = mtgTable(d.meetings || []); } catch (err) { document.getElementById('meetings-table').innerHTML = renderError('Filter failed', err.message); } });
}

// ── Meeting detail ──────────────────────────────────────────────────────

async function renderMeetingDetail(id) {
  const data = await apiJson('/v1/meetings/' + id); const m = data.meeting, participants = data.participants||[], summary = data.summary||null, tasks = data.tasks||[], decisions = data.decisions||[], transcript = data.transcript||[];
  const pNames = participants.map((p,i) => p.lark_name||p.name||('Participant '+(i+1)));
  const words = transcript.reduce((s,c) => s+(c.text||'').split(/\s+/).length, 0);
  let h = `<a href="/admin/meetings" class="back-link">${icon('back',14)} Meetings</a>
    <div class="report-header"><h1>${esc(m.title)}</h1><div class="report-meta"><span>${icon('calendar',14)} ${fmtDate(m.created_at)}</span>${m.started_at?`<span>${icon('clock',14)} ${dur(m.started_at, m.ended_at||new Date().toISOString())}</span>`:''}<span>${icon('users',14)} ${participants.length}</span>${words?`<span>${icon('doc',14)} ${words.toLocaleString()} words</span>`:''}<span>${statusPill(m.status)}</span></div></div>`;
  if (summary) h += `<div class="report-section"><div class="rs-title">${icon('sparkle',14)} AI Summary</div><div class="summary-card"><p>${esc(summary)}</p></div></div>`;
  if (tasks.length) h += `<div class="report-section"><div class="rs-title">${icon('check',14)} Action Items</div><div class="task-list">${tasks.map(t => `<div class="task-row"><div class="task-check ${t.lark_task_id?'done':''}"></div><div class="task-text">${esc(t.title)}</div><div class="task-assignee">${t.assignee?av(t.assignee,'av-sm')+' '+esc(t.assignee):'<span style="color:var(--fg-4)">Unassigned</span>'}</div>${t.lark_task_id?'<span class="pill pill-green">Synced</span>':''}</div>`).join('')}</div></div>`;
  if (decisions.length) h += `<div class="report-section"><div class="rs-title">Decisions</div><div class="decision-list">${decisions.map(d => `<div class="decision-row"><div class="decision-dot"></div><div class="decision-text">${esc(d.text)}</div></div>`).join('')}</div></div>`;
  if (m.agenda) h += `<div class="report-section"><div class="rs-title">${icon('doc',14)} Agenda</div><div class="summary-card"><p>${esc(m.agenda)}</p></div></div>`;
  h += `<div class="report-section"><div class="rs-title">${icon('users',14)} Participants</div>${!participants.length?'<div class="empty-state" style="padding:24px"><p>No participants.</p></div>':`<div class="participants-grid">${participants.map((p,i)=>{const n=pNames[i],st=p.status||'invited';let cls='pill-neutral';if(st==='connected'||st==='joined')cls='pill-green';else if(st==='left'||st==='disconnected')cls='pill-amber';return `<div class="participant-card">${av(n)}<div style="flex:1;min-width:0"><div class="participant-name">${esc(n)}</div>${p.joined_at?`<div class="participant-detail">Joined ${fmtTime(p.joined_at)}</div>`:''}</div><span class="pill ${cls}">${esc(st[0].toUpperCase()+st.slice(1))}</span>${p.disconnect_count>0?`<span class="participant-detail" style="color:var(--amber)">${p.disconnect_count}x</span>`:''}</div>`;}).join('')}</div>`}</div>`;
  if (transcript.length) h += `<div class="report-section"><div class="rs-title">${icon('mic',14)} Transcript</div><div class="transcript-wrap"><div class="transcript-body">${transcript.map(c=>{const sp=c.speaker_name||'Unknown';return `<div class="t-entry"><div class="t-speaker" style="color:${avColor(sp)}">${esc(sp)}</div><div class="t-text">${esc(c.text)}</div></div>`;}).join('')}</div><div class="transcript-footer">${transcript.length} chunks &middot; ${words.toLocaleString()} words</div></div></div>`;
  if (!summary&&transcript.length>0&&m.status==='ended') h += `<div class="report-section"><div class="alert alert-amber"><div><div class="alert-title">AI Summary Pending</div><div class="alert-body">Connect an OpenAI account in Settings to enable AI processing.</div></div></div></div>`;
  if (!summary&&!tasks.length&&!decisions.length&&!transcript.length) h += `<div class="report-section"><div class="empty-state" style="padding:40px"><h3>No meeting data yet</h3><p>Transcript and AI output will appear once the meeting has activity.</p></div></div>`;
  if (m.status==='ended') h += `<div class="lark-sync-bar"><div style="display:flex;align-items:center;gap:14px"><div class="lark-logo">L</div><div><h4>Sync to Lark</h4><p>Push ${tasks.length} task${tasks.length!==1?'s':''}, summary, and transcript.</p></div></div><button class="btn btn-primary" id="sync-lark-btn">Sync All ${icon('arrow',12)}</button></div><div id="sync-result" style="margin-top:16px"></div>`;
  if (m.status==='scheduled') h += `<div style="margin-top:32px"><button class="btn btn-primary" id="start-meeting-btn">Start Meeting</button></div>`;
  if (m.status==='live') h += `<div style="margin-top:32px"><button class="btn btn-danger" id="end-meeting-btn">End Meeting</button></div>`;
  return h;
}
function bindMeetingDetail(id) {
  const sync = document.getElementById('sync-lark-btn');
  if (sync) sync.addEventListener('click', async () => { sync.disabled = true; sync.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Syncing...'; try { const d = await apiJson('/v1/meetings/'+id+'/sync-to-lark',{method:'POST'}); document.getElementById('sync-result').innerHTML = `<div class="alert alert-green"><div class="alert-title">Synced</div><div class="alert-body">${d.tasks_synced!=null?d.tasks_synced+' tasks':''}${d.doc_id?' · Doc created':''}${d.messages_sent?' · '+d.messages_sent+' messages':''}</div></div>`; sync.textContent = 'Synced'; sync.style.background = 'var(--green)'; sync.style.borderColor = 'var(--green)'; } catch(e) { document.getElementById('sync-result').innerHTML = renderError('Sync failed', e.message); sync.disabled = false; sync.innerHTML = 'Sync All '+icon('arrow',12); } });
  const start = document.getElementById('start-meeting-btn');
  if (start) start.addEventListener('click', async () => { start.disabled = true; start.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Starting...'; try { await apiJson('/v1/meetings/'+id+'/start',{method:'POST'}); render(); } catch(e) { start.disabled = false; start.textContent = 'Start Meeting'; alert('Failed: '+e.message); } });
  const end = document.getElementById('end-meeting-btn');
  if (end) end.addEventListener('click', async () => { if(!confirm('End this meeting?'))return; end.disabled = true; end.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Ending...'; try { await apiJson('/v1/meetings/'+id+'/end',{method:'POST'}); render(); } catch(e) { end.disabled = false; end.textContent = 'End Meeting'; alert('Failed: '+e.message); } });
}

// ── New Meeting ─────────────────────────────────────────────────────────

async function renderMeetingNew() {
  const orgData = await fetchOrg(); let members = [];
  if (orgData&&orgData.org) try { const d = await apiJson('/v1/orgs/'+orgData.org.id+'/members'); members = d.members||[]; } catch {}
  return `<a href="/admin/meetings" class="back-link">${icon('back',14)} Meetings</a>
    <div class="page-title">New Meeting</div><div class="page-sub">Create and invite participants</div>
    <div style="max-width:520px">
      <div class="form-group"><label class="form-label">Title</label><input type="text" class="form-input" id="meeting-title" placeholder="Sprint Planning — Week 22" required/></div>
      <div class="form-group"><label class="form-label">Agenda (optional)</label><textarea class="form-input" id="meeting-agenda" rows="3" placeholder="Topics to discuss..." style="resize:vertical"></textarea></div>
      <div class="form-group"><label class="form-label">Participants</label><div id="participant-list" style="display:flex;flex-direction:column;gap:4px;margin-top:4px">${members.map(m=>{const n=m.lark_name||m.account_id;return `<label style="display:flex;align-items:center;gap:10px;padding:10px 12px;border:1px solid var(--border);border-radius:var(--radius);cursor:pointer;transition:border-color 0.15s"><input type="checkbox" value="${m.account_id}" class="participant-cb" style="accent-color:var(--accent);width:16px;height:16px"/>${av(n,'av-sm')} <span style="font-size:13px;font-weight:500;flex:1">${esc(n)}</span><span style="font-size:11px;color:var(--fg-4)">${esc(m.lark_department||'')}</span></label>`;}).join('')}</div>${!members.length?'<p class="form-hint">No team members. Sync via Lark first.</p>':''}</div>
      <div id="create-error" style="display:none" class="alert alert-red"></div>
      <button class="btn btn-primary" id="create-meeting-btn" style="margin-top:8px">Create Meeting</button>
    </div>`;
}
function bindMeetingNew() {
  document.getElementById('create-meeting-btn').addEventListener('click', async () => {
    const title = document.getElementById('meeting-title').value.trim(), agenda = document.getElementById('meeting-agenda').value.trim(), err = document.getElementById('create-error'), btn = document.getElementById('create-meeting-btn');
    if (!title) { err.textContent = 'Title is required.'; err.style.display = 'block'; return; }
    const ids = Array.from(document.querySelectorAll('.participant-cb:checked')).map(c => c.value);
    btn.disabled = true; btn.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Creating...'; err.style.display = 'none';
    try { const d = await apiJson('/v1/meetings', { method: 'POST', body: JSON.stringify({ title, agenda: agenda||null, participant_ids: ids }) }); navigate('/admin/meetings/'+d.meeting.id); }
    catch (e) { btn.disabled = false; btn.textContent = 'Create Meeting'; err.textContent = e.message; err.style.display = 'block'; }
  });
}

// ── Team ────────────────────────────────────────────────────────────────

async function renderTeam() {
  const orgData = await fetchOrg();
  if (!orgData||!orgData.org) return '<div class="page-title">Team</div><div class="empty-state"><h3>No organization</h3></div>';
  const org = orgData.org; let members = []; try { const d = await apiJson('/v1/orgs/'+org.id+'/members'); members = d.members||[]; } catch {}
  return `<div class="page-title">Team</div><div class="page-sub">${esc(org.name)} &middot; ${members.length} member${members.length!==1?'s':''}</div>
    ${!members.length?'<div class="empty-state"><h3>No members</h3></div>':`<div class="panel"><table><thead><tr><th>Name</th><th>Department</th><th>Role</th><th>Joined</th></tr></thead><tbody>${members.map(m=>{const n=m.lark_name||('User '+(m.account_id?m.account_id.substring(0,8):'?'));return `<tr style="cursor:default"><td><div style="display:flex;align-items:center;gap:10px">${av(n,'av-sm')} ${esc(n)}</div></td><td>${esc(m.lark_department||'--')}</td><td>${rolePill(m.role)}</td><td>${fmtDate(m.joined_at)}</td></tr>`;}).join('')}</tbody></table></div>`}`;
}

// ── Settings ────────────────────────────────────────────────────────────

async function renderSettings() {
  const [orgData, oaiData] = await Promise.all([fetchOrg(), apiJson('/v1/openai/status').catch(()=>({connected:false}))]);
  const org = orgData&&orgData.org?orgData.org:null;
  const roles = org&&org.meeting_creator_roles?org.meeting_creator_roles:[];
  const all = ['COMPANY_ADMIN','MANAGER','MEMBER'];
  return `<div class="page-title">Settings</div><div class="page-sub">Manage your organization</div>
    <div class="settings-grid">
      ${org?`<div><div class="settings-section-title">Organization</div><div class="settings-card"><div style="display:grid;grid-template-columns:1fr 1fr;gap:16px"><div><div style="font-size:12px;color:var(--fg-3);margin-bottom:4px">Name</div><div style="font-size:14px;font-weight:500">${esc(org.name)}</div></div><div><div style="font-size:12px;color:var(--fg-3);margin-bottom:4px">Slug</div><div style="font-size:14px;font-family:var(--mono)">${esc(org.slug)}</div></div></div><div style="margin-top:12px"><div style="font-size:12px;color:var(--fg-3);margin-bottom:4px">Role</div>${rolePill(org.role)}</div></div></div>`:''}
      ${org?`<div><div class="settings-section-title">Meeting Creator Roles</div><div class="settings-card"><p style="font-size:12px;color:var(--fg-3);margin-bottom:12px">Which roles can create meetings.</p><div class="checkbox-group">${all.map(r=>{const c=Array.isArray(roles)&&roles.some(x=>x===r||x.toUpperCase()===r);return `<label class="checkbox-item"><input type="checkbox" value="${r}" ${c?'checked':''}/> ${esc(r)}</label>`;}).join('')}</div></div></div>`:''}
      <div><div class="settings-section-title">Appearance</div><div class="settings-card"><div style="display:flex;align-items:center;justify-content:space-between"><div><div style="font-size:14px;font-weight:500">Dark Mode</div><div style="font-size:12px;color:var(--fg-3);margin-top:2px">Toggle between light and dark theme</div></div><button class="btn btn-secondary" onclick="toggleTheme();updateThemeIcon()">${icon(getTheme()==='dark'?'sun':'moon',14)} ${getTheme()==='dark'?'Light Mode':'Dark Mode'}</button></div></div></div>
      <div><div class="settings-section-title">OpenAI Integration</div><div class="openai-card ${oaiData.connected?'openai-connected':''}" id="openai-section">${renderOpenai(oaiData)}</div></div>
    </div>`;
}
function renderOpenai(data) {
  if (data.connected) return `<div style="display:flex;align-items:center;justify-content:space-between"><div style="display:flex;align-items:center;gap:10px"><span class="openai-dot on"></span><div><div style="font-size:14px;font-weight:500">Connected</div><div style="font-size:12px;color:var(--fg-3);margin-top:2px">${data.plan_type?'Plan: '+esc(data.plan_type):''}${data.connected_at?' · Since '+fmtDate(data.connected_at):''}</div></div></div><button class="btn btn-danger btn-sm" id="openai-disconnect-btn">Disconnect</button></div>`;
  return `<div style="display:flex;align-items:center;gap:10px;margin-bottom:16px"><span class="openai-dot off"></span><div><div style="font-size:14px;font-weight:500">Not Connected</div><div style="font-size:12px;color:var(--fg-3);margin-top:2px">Connect OpenAI for AI meeting summaries.</div></div></div><button class="btn btn-primary" id="openai-connect-btn">Connect OpenAI</button><div id="openai-connect-flow" style="display:none;margin-top:16px"></div>`;
}
function bindSettings() {
  const conn = document.getElementById('openai-connect-btn');
  if (conn) conn.addEventListener('click', async () => {
    conn.disabled = true; conn.innerHTML = '<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Connecting...';
    try { const data = await apiJson('/v1/openai/connect',{method:'POST'}); const flow = document.getElementById('openai-connect-flow'); flow.style.display = 'block';
      flow.innerHTML = `<div class="settings-card"><div class="form-group"><label class="form-label">Authorization URL</label><input type="text" class="form-input" value="${esc(data.auth_url)}" readonly onclick="this.select()" style="font-size:11px;font-family:var(--mono)"/><p class="form-hint">Open this URL in your browser.</p></div><div class="form-group"><label class="form-label">Callback URL</label><input type="text" class="form-input" id="openai-code" placeholder="Paste the full callback URL"/><p class="form-hint">After logging in, copy the entire URL.</p></div><div class="form-group"><label class="form-label">Plan Type</label><select class="form-input" id="openai-plan-type"><option value="">Select plan...</option><option value="free">Free</option><option value="plus">Plus</option><option value="pro">Pro</option><option value="team">Team</option><option value="enterprise">Enterprise</option></select></div><button class="btn btn-primary" id="openai-complete-btn">Complete Connection</button></div>`;
      const v = data.code_verifier;
      document.getElementById('openai-complete-btn').addEventListener('click', async () => { let raw = document.getElementById('openai-code').value.trim(), plan = document.getElementById('openai-plan-type').value; if(!raw){alert('Paste the callback URL.');return;} let code=raw; try{const u=new URL(raw);const c=u.searchParams.get('code');if(c)code=c;}catch{} const cb=document.getElementById('openai-complete-btn'); cb.disabled=true; cb.innerHTML='<div class="spinner" style="width:14px;height:14px;border-width:2px"></div> Completing...'; try{await apiJson('/v1/openai/complete',{method:'POST',body:JSON.stringify({code,code_verifier:v,plan_type:plan||null})});cachedOrg=null;render();}catch(e){cb.disabled=false;cb.textContent='Complete Connection';alert('Failed: '+e.message);} });
      conn.textContent = 'Authorize in Browser'; conn.disabled = false; window.open(data.auth_url, '_blank');
    } catch (e) { conn.disabled = false; conn.textContent = 'Connect OpenAI'; alert('Failed: '+e.message); }
  });
  const disc = document.getElementById('openai-disconnect-btn');
  if (disc) disc.addEventListener('click', async () => { if(!confirm('Disconnect OpenAI?'))return; disc.disabled=true; disc.innerHTML='<div class="spinner" style="width:14px;height:14px;border-width:2px"></div>'; try{await api('/v1/openai/disconnect',{method:'DELETE'});cachedOrg=null;render();}catch(e){disc.disabled=false;disc.textContent='Disconnect';alert('Failed: '+e.message);} });
}

function renderError(title, msg) { return `<div class="error-state"><h3>${esc(title)}</h3><p>${esc(msg)}</p></div>`; }
