// 2D の HUD（文字・パネル・図）を描く共通部品。intro.html と sim-real.html が読む。
// initHud(canvas) のあと、グローバルの g（2D コンテキスト）と下の関数が使える。
const W = 1920, H = 1080;
const clamp = (x, a = 0, b = 1) => Math.min(b, Math.max(a, x));
const ss = x => { x = clamp(x); return x * x * (3 - 2 * x); };
const lerp = (a, b, k) => a + (b - a) * k;
const A = (t, s, d = 0.6) => ss((t - s) / d);            // 時刻 s から d 秒かけて 0→1
const JP = '"Noto Sans CJK JP","Noto Sans JP",sans-serif', MONO = '"DejaVu Sans Mono",monospace';
const CY = '#35e6ff', GR = '#6dffb0', AM = '#ffc04d', RD = '#ff5a5a', PU = '#c79bff', WH = '#eef6fb', DIM = '#93aebd';
let g = null;
function initHud(canvas) { g = canvas.getContext('2d'); return g; }

function txt(s, x, y, size, color = WH, o = {}) {
  g.font = `${o.w || 500} ${size}px ${o.mono ? MONO : JP}`;
  g.textAlign = o.al || 'left'; g.textBaseline = o.bl || 'alphabetic';
  g.shadowColor = o.glow || 'rgba(0,0,0,0.85)'; g.shadowBlur = o.glow ? (o.gb || 16) : 6;
  g.fillStyle = color; g.fillText(s, x, y); g.shadowBlur = 0;
  return g.measureText(s).width;
}
function panel(x, y, w, h, o = {}) {
  const c = o.c || CY, k = o.cut ?? 14;
  g.fillStyle = o.bg || 'rgba(6,11,18,0.80)';
  g.beginPath(); g.moveTo(x + k, y); g.lineTo(x + w, y); g.lineTo(x + w, y + h - k); g.lineTo(x + w - k, y + h); g.lineTo(x, y + h); g.lineTo(x, y + k); g.closePath(); g.fill();
  const p = g.globalAlpha; g.globalAlpha = p * (o.sa ?? 0.5); g.strokeStyle = c; g.lineWidth = 1.5; g.stroke(); g.globalAlpha = p;
  g.fillStyle = c; g.fillRect(x, y + k, 3, Math.min(h - k, o.bar ?? 44));
}
function chip(s, x, y, c = CY, size = 24, o = {}) {
  g.font = `700 ${size}px ${o.mono ? MONO : JP}`;
  const w = g.measureText(s).width + size * 1.1, h = size * 1.75;
  g.fillStyle = o.fill ? c : 'rgba(6,11,18,0.85)'; g.fillRect(x, y, w, h);
  g.strokeStyle = c; g.lineWidth = 2; g.strokeRect(x, y, w, h);
  g.fillStyle = o.fill ? '#04121a' : c; g.textAlign = 'left'; g.textBaseline = 'middle'; g.shadowBlur = 0;
  g.fillText(s, x + size * 0.55, y + h / 2 + 1);
  return w;
}
function line(x1, y1, x2, y2, c = CY, lw = 2, dash) {
  g.strokeStyle = c; g.lineWidth = lw; g.setLineDash(dash || []);
  g.beginPath(); g.moveTo(x1, y1); g.lineTo(x2, y2); g.stroke(); g.setLineDash([]);
}
function withA(a, fn) { if (a <= 0.002) return; const p = g.globalAlpha; g.globalAlpha = p * clamp(a); fn(); g.globalAlpha = p; }
function slide(a, dx, fn) { withA(a, () => { g.save(); g.translate((1 - a) * dx, 0); fn(); g.restore(); }); }
// 見出し（番号つきの英字ラベル + 日本語の見出し）
function heading(en, jp, a, x = 1000, y = 150, c = CY, maxW = 830) {
  g.font = `700 50px ${JP}`;
  const size = Math.min(50, Math.floor(50 * maxW / g.measureText(jp).width));   // 幅に収まるよう縮める
  slide(a, 30, () => { txt(en, x, y, 24, c, {mono: true, w: 700}); g.fillStyle = c; g.fillRect(x, y + 16, 56, 3); txt(jp, x, y + 84, size, WH, {w: 700}); });
}
// 3D 上の点 p=[x,y] から引き出し線つきのラベルを出す
function tag(s, p, c, dx = 46, dy = -70, o = {}) {
  const [x, y] = p, ex = x + dx, ey = y + dy, len = o.len ?? (g.font = `700 ${o.size || 22}px ${o.mono === false ? JP : MONO}`, g.measureText(s).width + 14);
  line(x, y, ex, ey, c, 1.5); line(ex, ey, ex + (dx >= 0 ? len : -len), ey, c, 1.5);
  g.fillStyle = c; g.beginPath(); g.arc(x, y, 4.5, 0, 7); g.fill();
  txt(s, dx >= 0 ? ex + 6 : ex - 6, ey - 9, o.size || 22, c, {mono: o.mono !== false, w: 700, al: dx >= 0 ? 'left' : 'right'});
}
// 「項目 | 説明」の行
function kv(k, v, x, y, w, c = CY, o = {}) {
  panel(x, y, w, o.h || 78, {c, bar: 78});
  txt(k, x + 24, y + 48, o.ks || 24, c, {w: 700});
  txt(v, x + (o.kw || 200), y + 48, o.vs || 24, WH);
}
// 画面共通の枠（左上のタイトル、右上の状態、下の進捗、周辺減光、冒頭と最後のフェード）
function frameChrome(t, o) {
  const vg = g.createRadialGradient(W / 2, H / 2, H * 0.4, W / 2, H / 2, H * 0.98);
  vg.addColorStop(0, 'rgba(0,0,0,0)'); vg.addColorStop(1, 'rgba(0,0,0,0.55)'); g.fillStyle = vg; g.fillRect(0, 0, W, H);
  const cur = o.scenes.find(x => t >= x.s && t < x.e) || o.scenes[o.scenes.length - 1];
  withA(Math.min(A(t, 1.2, 1), 1 - A(t, o.duration - 6, 1)) * 0.9, () => {
    txt(o.title, 70, 80, 20, CY, {mono: true, w: 700});
    txt('/ ' + cur.label, 70 + (g.font = `700 20px ${MONO}`, g.measureText(o.title).width) + 16, 80, 20, DIM, {mono: true});
    g.fillStyle = 'rgba(53,230,255,0.18)'; g.fillRect(70, H - 58, 360, 2); g.fillStyle = CY; g.fillRect(70, H - 58, 360 * t / o.duration, 2);
    o.scenes.forEach(x => { g.fillStyle = t >= x.s ? CY : 'rgba(53,230,255,0.4)'; g.fillRect(70 + 360 * x.s / o.duration - 1, H - 63, 2, 12); });
    if (o.status) { const [s, c] = o.status; const w = txt(s, W - 70, 80, 20, c, {mono: true, w: 700, al: 'right'}); g.fillStyle = c; g.beginPath(); g.arc(W - 70 - w - 16, 73, 5.5, 0, 7); g.fill(); }
  });
}
function frameFade(t, duration) {
  const fa = Math.max(1 - A(t, 0, 1.0), A(t, duration - 1.0, 1.0));
  if (fa > 0) { g.fillStyle = `rgba(0,0,0,${fa})`; g.fillRect(0, 0, W, H); }
}
// 時刻つきキー [t, v0, v1, ...] を滑らかにつなぐ（非一様 Catmull-Rom。キーで止まらない）
function splineAt(K, t) {
  let i = 0; while (i < K.length - 2 && t > K[i + 1][0]) i++;
  const a = K[i], b = K[i + 1], p = K[Math.max(0, i - 1)], n = K[Math.min(K.length - 1, i + 2)];
  const dt = b[0] - a[0], u = clamp((t - a[0]) / dt), u2 = u * u, u3 = u2 * u, out = [];
  for (let j = 1; j < a.length; j++) {
    const ma = (b[j] - p[j]) / (b[0] - p[0]) * dt, mb = (n[j] - a[j]) / (n[0] - a[0]) * dt;
    out.push((2 * u3 - 3 * u2 + 1) * a[j] + (u3 - 2 * u2 + u) * ma + (-2 * u3 + 3 * u2) * b[j] + (u3 - u2) * mb);
  }
  return out;
}
// splineAt と同じ使い方。行き過ぎ（オーバーシュート）しない接線を使うので、大きく移動するキーが混ざっても暴れない
function splineMono(K, t) {
  let i = 0; while (i < K.length - 2 && t > K[i + 1][0]) i++;
  const a = K[i], b = K[i + 1], p = K[i - 1], n = K[i + 2];
  const dt = b[0] - a[0], u = clamp((t - a[0]) / dt), u2 = u * u, u3 = u2 * u, out = [];
  const tan = (d0, d1) => (d0 * d1 <= 0 ? 0 : 2 * d0 * d1 / (d0 + d1));
  for (let j = 1; j < a.length; j++) {
    const d = (b[j] - a[j]) / dt;
    const ma = p ? tan((a[j] - p[j]) / (a[0] - p[0]), d) : d, mb = n ? tan(d, (n[j] - b[j]) / (n[0] - b[0])) : d;
    out.push((2 * u3 - 3 * u2 + 1) * a[j] + (u3 - 2 * u2 + u) * ma * dt + (-2 * u3 + 3 * u2) * b[j] + (u3 - u2) * mb * dt);
  }
  return out;
}
