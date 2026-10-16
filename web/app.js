"use strict";
// SDV browser front-end. The simulation itself runs in sdv_core.wasm (Rust).

const $ = (id) => document.getElementById(id);
const IDX = { t:0, x:1, y:2, yaw:3, v:4, yawRate:5, wheel:6, thr:7, brk:8, str:9, acc:10, dec:11, steerCmd:12, estop:13, state:14 };
const STATE_NAMES = ["Normal", "Degraded", "EStop"];
const HISTORY_S = 10;

let wasm, mem;
const trail = [];           // {x,y}
const hist = [];            // {t,v,steer}
let paused = false, scenario = false, estop = false, speed = 1;
let last = null, acc = 0;
const keys = new Set();

function out() { return new Float32Array(mem.buffer, wasm.sdv_out_ptr(), wasm.sdv_out_len()); }

function applyInput() {
  wasm.sdv_set_input(+$("thr").value, +$("brk").value, +$("str").value);
  wasm.sdv_set_estop(estop ? 1 : 0);
}

function resetSim() {
  wasm.sdv_reset();
  wasm.sdv_use_scenario(scenario ? 1 : 0);
  trail.length = 0; hist.length = 0;
  applyInput();
  wasm.sdv_step(1);
}

function setSlider(id, v) { $(id).value = v; $(id + "O").value = (+v).toFixed(2); }
for (const id of ["thr", "brk", "str"]) $(id).addEventListener("input", () => { $(id + "O").value = (+$(id).value).toFixed(2); });
$("spd").addEventListener("input", () => { speed = +$("spd").value; $("spdO").value = speed.toFixed(1) + "x"; });

function setEstop(on) { estop = on; $("estop").classList.toggle("on", on); }
$("estop").addEventListener("click", () => setEstop(!estop));
$("scn").addEventListener("click", () => {
  scenario = !scenario; $("scn").classList.toggle("on", scenario);
  for (const id of ["thr", "brk", "str"]) $(id).disabled = scenario;
  resetSim();
});
$("reset").addEventListener("click", () => { setEstop(false); resetSim(); });
$("pause").addEventListener("click", () => { paused = !paused; $("pause").classList.toggle("on", paused); $("pause").textContent = paused ? "再開" : "一時停止"; });

addEventListener("keydown", (e) => {
  if (e.code === "Space") { e.preventDefault(); if (!e.repeat) setEstop(!estop); return; }
  keys.add(e.code);
  if (e.code.startsWith("Arrow")) e.preventDefault();
});
addEventListener("keyup", (e) => keys.delete(e.code));

function keyboardControl(dt) {
  if (scenario) return;
  let thr = +$("thr").value, brk = +$("brk").value, str = +$("str").value;
  const up = keys.has("ArrowUp") || keys.has("KeyW"), down = keys.has("ArrowDown") || keys.has("KeyS");
  const left = keys.has("ArrowLeft") || keys.has("KeyA"), right = keys.has("ArrowRight") || keys.has("KeyD");
  thr = Math.min(1, Math.max(0, thr + (up ? 1.5 : -2.5) * dt));
  brk = Math.min(1, Math.max(0, brk + (down ? 2.5 : -3.5) * dt));
  if (left || right) str = Math.min(1, Math.max(-1, str + (right ? 1 : -1) * 2.5 * dt));
  else if (up || down) str *= Math.max(0, 1 - 4 * dt); // self-centre while driving
  // Keys only take over the sliders while a key is held; otherwise manual slider values stay.
  if (keys.size) { setSlider("thr", thr); setSlider("brk", brk); setSlider("str", str); }
}

// ---------- rendering ----------
const css = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
function fit(c) {
  const r = c.getBoundingClientRect(), d = devicePixelRatio || 1;
  const w = Math.round(r.width * d), h = Math.round(r.height * d);
  if (c.width !== w || c.height !== h) { c.width = w; c.height = h; }
  return d;
}

function drawView(o) {
  const c = $("view"), d = fit(c), g = c.getContext("2d");
  const W = c.width, H = c.height, S = 220 * d; // px per metre
  g.fillStyle = css("--bg"); g.fillRect(0, 0, W, H);
  const cx = W / 2, cy = H / 2;
  const sx = (x) => cx + (x - o[IDX.x]) * S, sy = (y) => cy - (y - o[IDX.y]) * S;

  // grid (0.25 m) follows the world so motion is visible
  g.strokeStyle = css("--grid"); g.lineWidth = d; g.beginPath();
  const step = 0.25, x0 = Math.floor((o[IDX.x] - cx / S) / step) * step, y0 = Math.floor((o[IDX.y] - cy / S) / step) * step;
  for (let x = x0; sx(x) < W + 1; x += step) { g.moveTo(sx(x), 0); g.lineTo(sx(x), H); }
  for (let y = y0; sy(y) > -1; y += step) { g.moveTo(0, sy(y)); g.lineTo(W, sy(y)); }
  g.stroke();

  // origin
  g.fillStyle = css("--sub"); g.beginPath(); g.arc(sx(0), sy(0), 4 * d, 0, 7); g.fill();

  // trail
  g.strokeStyle = css("--trail"); g.lineWidth = 3 * d; g.lineJoin = "round"; g.beginPath();
  trail.forEach((p, i) => (i ? g.lineTo(sx(p.x), sy(p.y)) : g.moveTo(sx(p.x), sy(p.y))));
  g.stroke();

  // car (bicycle model: 0.20 m wheelbase)
  const L = 0.20 * S, Wd = 0.12 * S, yaw = o[IDX.yaw], steer = o[IDX.steerCmd];
  g.save(); g.translate(cx, cy); g.rotate(-yaw);
  g.fillStyle = estop || o[IDX.state] === 2 ? css("--danger") : css("--car");
  g.beginPath(); g.roundRect(-L * 0.25, -Wd / 2, L * 1.5, Wd, 6 * d); g.fill();
  // wheels: rear fixed, front steered
  g.fillStyle = "#111";
  for (const [wx, ang] of [[0, 0], [L, -steer]]) for (const wy of [-Wd / 2, Wd / 2]) {
    g.save(); g.translate(wx, wy); g.rotate(ang);
    g.fillRect(-L * 0.12, -2 * d, L * 0.24, 4 * d); g.restore();
  }
  g.fillStyle = "#fff"; g.beginPath(); g.moveTo(L * 1.25, 0); g.lineTo(L * 0.95, -Wd * 0.22); g.lineTo(L * 0.95, Wd * 0.22); g.fill();
  g.restore();

  g.fillStyle = css("--sub"); g.font = `${11 * d}px system-ui`;
  g.fillText("グリッド 0.25 m", 8 * d, H - 8 * d);
}

// ステアリングホイール: 舵角(steerCmd, 最大±0.4rad)を ±450° の見た目回転に対応させる（表示用の比率）
function drawWheel(o) {
  const c = $("wheel"), d = fit(c), g = c.getContext("2d");
  const W = c.width, H = c.height, r = Math.min(W, H) * 0.38;
  const steer = o[IDX.steerCmd], rot = (steer / 0.4) * (450 * Math.PI / 180);
  g.clearRect(0, 0, W, H);
  g.save(); g.translate(W / 2, H / 2 - 6 * d); g.rotate(rot);
  g.strokeStyle = css("--ink"); g.fillStyle = css("--ink"); g.lineCap = "round";
  g.lineWidth = r * 0.16; g.beginPath(); g.arc(0, 0, r, 0, 7); g.stroke();       // リム
  g.lineWidth = r * 0.12; g.beginPath(); g.moveTo(-r, 0); g.lineTo(r, 0);        // 左右スポーク
  g.moveTo(0, 0); g.lineTo(0, r); g.stroke();                                    // 下スポーク
  g.beginPath(); g.arc(0, 0, r * 0.2, 0, 7); g.fill();                           // ハブ
  g.fillStyle = css("--accent"); g.beginPath(); g.roundRect(-r * 0.06, -r - r * 0.12, r * 0.12, r * 0.24, 3 * d); g.fill(); // 12時マーク
  g.restore();
  g.fillStyle = css("--sub"); g.font = `${11 * d}px system-ui`; g.textAlign = "center";
  g.fillText(`${(rot * 180 / Math.PI).toFixed(0)}°`, W / 2, H - 4 * d); g.textAlign = "start";
}

function drawChart() {
  const c = $("chart"), d = fit(c), g = c.getContext("2d");
  const W = c.width, H = c.height;
  g.clearRect(0, 0, W, H);
  g.strokeStyle = css("--line"); g.lineWidth = d; g.strokeRect(0.5, 0.5, W - 1, H - 1);
  if (hist.length < 2) return;
  const t1 = hist[hist.length - 1].t, t0 = Math.max(0, t1 - HISTORY_S);
  const X = (t) => ((t - t0) / HISTORY_S) * W;
  const line = (key, color, lo, hi) => {
    g.strokeStyle = color; g.lineWidth = 2 * d; g.beginPath();
    hist.forEach((p, i) => { const y = H - ((p[key] - lo) / (hi - lo)) * (H - 8 * d) - 4 * d; i ? g.lineTo(X(p.t), y) : g.moveTo(X(p.t), y); });
    g.stroke();
  };
  line("v", css("--accent"), 0, 3.2);
  line("steer", css("--ok"), -0.45, 0.45);
  g.font = `${11 * d}px system-ui`;
  g.fillStyle = css("--accent"); g.fillText("v (0–3 m/s)", 8 * d, 14 * d);
  g.fillStyle = css("--ok"); g.fillText("steer cmd (±0.4 rad)", 90 * d, 14 * d);
}

function updateHud(o) {
  const deg = (r) => (r * 180 / Math.PI).toFixed(1);
  $("s_t").textContent = o[IDX.t].toFixed(2);
  $("s_v").textContent = o[IDX.v].toFixed(2);
  $("s_x").textContent = o[IDX.x].toFixed(2);
  $("s_y").textContent = o[IDX.y].toFixed(2);
  $("s_yaw").textContent = deg(o[IDX.yaw]);
  $("s_yr").textContent = deg(o[IDX.yawRate]);
  $("s_w").textContent = o[IDX.wheel].toFixed(1);
  $("s_sc").textContent = deg(o[IDX.steerCmd]);
  $("s_ac").textContent = o[IDX.acc].toFixed(2);
  $("s_dc").textContent = o[IDX.dec].toFixed(2);
  const st = STATE_NAMES[o[IDX.state]] || "?";
  $("state").textContent = st; $("state").classList.toggle("estop", o[IDX.state] === 2);
  if (scenario) { setSlider("thr", o[IDX.thr]); setSlider("brk", o[IDX.brk]); setSlider("str", o[IDX.str]); }
}

function frame(ts) {
  requestAnimationFrame(frame);
  const dt = last == null ? 0 : Math.min(0.1, (ts - last) / 1000);
  last = ts;
  if (!paused) {
    keyboardControl(dt);
    if (!scenario) applyInput(); else wasm.sdv_set_estop(estop ? 1 : 0);
    acc += dt * speed;
    let n = Math.min(Math.floor(acc / 0.01), 50);
    acc -= n * 0.01;
    while (n-- > 0) {
      wasm.sdv_step(1);
      const o = out();
      trail.push({ x: o[IDX.x], y: o[IDX.y] });
      hist.push({ t: o[IDX.t], v: o[IDX.v], steer: o[IDX.steerCmd] });
    }
    if (trail.length > 6000) trail.splice(0, trail.length - 6000);
    while (hist.length && hist[0].t < hist[hist.length - 1].t - HISTORY_S - 1) hist.shift();
  }
  const o = out();
  drawView(o); drawWheel(o); drawChart(); updateHud(o);
}

(async () => {
  try {
    const res = await fetch("sdv_core.wasm");
    const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    wasm = instance.exports; mem = wasm.memory;
    resetSim();
    requestAnimationFrame(frame);
  } catch (e) {
    const el = $("err"); el.style.display = "block";
    el.textContent = "WASMの読み込みに失敗しました: " + e.message + "（ファイルを直接開かず、HTTP経由で表示してください）";
  }
})();
