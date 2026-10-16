"use strict";
// SDV browser front-end. The simulation itself runs in sdv_core.wasm (Rust).

const $ = (id) => document.getElementById(id);
const IDX = { t:0, x:1, y:2, yaw:3, v:4, yawRate:5, wheel:6, thr:7, brk:8, str:9, acc:10, dec:11, steerCmd:12, estop:13, state:14 };
const SAFETY = [
  { cls: "normal",   ico: "●", lbl: "NORMAL",   sub: "正常" },
  { cls: "degraded", ico: "▲", lbl: "DEGRADED", sub: "縮退運転" },
  { cls: "estop",    ico: "■", lbl: "E-STOP",   sub: "非常停止" },
];
const HISTORY_S = 10;
const SILENT0 = 15;            // 出力バッファ: SWCごとの無応答時間[ms] (engine, brake, steering, vehicledynamics)
let TIMEOUT_MS = 1000;         // ウォッチドッグのタイムアウト（Configの safety.heartbeat_timeout_ms に追従）
let vMax = 3.0, steerMax = 0.4; // グラフ・ハンドル表示のスケール（Configの車速上限・最大舵角に追従）
const FS0 = 19;                // 出力バッファ: ECU(engine, brake, steering)ごとのフェイルセーフ（bit0: 入力途絶, bit1: Safety途絶）
const stalled = [false, false, false, false];
let vcuStalled = false;
const bus = { delayTicks: 0, loss: 0, down: false };

let wasm, mem;
const trail = [];           // {x,y} (視点側の短い軌跡、最大6000点)
const path = [];            // 全体マップ用の間引き済み軌跡 {x,y}
let pathDist = 0;
const hist = [];            // {t,v,steer}
let paused = false, scenario = false, estop = false, speed = 1;
let last = null, acc = 0;
const keys = new Set();

function out() { return new Float32Array(mem.buffer, wasm.sdv_out_ptr(), wasm.sdv_out_len()); }

// 符号規約: モデル(C++/Rust)の steer は「正 = 左旋回(反時計回り)」。
// GUIのスライダー/キーは直感どおり「右 = 正」なので、モデルへ渡すときに符号を反転する。
function applyInput() {
  wasm.sdv_set_input(+$("thr").value, +$("brk").value, -Number($("str").value));
  wasm.sdv_set_estop(estop ? 1 : 0);
}

function resetSim() {
  wasm.sdv_reset();
  stalled.fill(false); vcuStalled = false;
  bus.delayTicks = 0; bus.loss = 0; bus.down = false;
  $("bdelay").value = 0; $("bloss").value = 0; syncBusUi();
  wasm.sdv_use_scenario(scenario ? 1 : 0);
  trail.length = 0; hist.length = 0; clearPath();
  applyInput();
  wasm.sdv_step(1);
}

// ECU構成図（Rust側の sdv_set_stall / sdv_set_vcu_stall / sdv_set_bus に対応）
// index: 0 engine, 1 brake, 2 steering, 3 plant(vehicledynamics)。VCUは別枠。
function makeNode(parent, name, sub, onClick) {
  const b = document.createElement("button");
  b.className = "swc"; b.setAttribute("aria-pressed", "false");
  b.innerHTML = `<span class="nm">${name}</span><span class="st">正常</span><span class="bar"><i></i></span>`;
  b.title = sub; b.addEventListener("click", onClick);
  parent.appendChild(b);
  return b;
}
const vcuEl = makeNode($("n_vcu"), "VCU（DriverInput · Safety）", "車両制御ユニット / ゲートウェイ", () => {
  vcuStalled = !vcuStalled; wasm.sdv_set_vcu_stall(vcuStalled ? 1 : 0);
});
const NODE_DEFS = [["Engine ECU", 0, "Engine"], ["Brake ECU", 1, "Brake"], ["Steering ECU", 2, "Steering"]];
const ecuEls = NODE_DEFS.map(([name, i, sub]) => makeNode($("ecus"), name, sub, () => {
  stalled[i] = !stalled[i]; wasm.sdv_set_stall(i, stalled[i] ? 1 : 0);
}));
const plantEl = makeNode($("n_plant"), "Plant（VehicleDynamics）", "制御対象。ECUではない", () => {
  stalled[3] = !stalled[3]; wasm.sdv_set_stall(3, stalled[3] ? 1 : 0);
});
function applyBus() { wasm.sdv_set_bus(bus.delayTicks, bus.loss * 10, bus.down ? 1 : 0); }
function syncBusUi() {
  $("bdelayO").value = bus.delayTicks * 10 + " ms"; $("blossO").value = bus.loss + " %";
  $("bdown").classList.toggle("on", bus.down); $("bdown").setAttribute("aria-pressed", bus.down);
  const bl = $("busline");
  bl.className = "busline" + (bus.down ? " down" : (bus.delayTicks || bus.loss) ? " degraded" : "");
  $("busLbl").textContent = bus.down ? "車載バス 断" : (bus.delayTicks || bus.loss)
    ? `車載バス 遅延 ${bus.delayTicks * 10} ms / 欠損 ${bus.loss} %` : "車載バス 正常";
}
$("bdelay").addEventListener("input", () => { bus.delayTicks = +$("bdelay").value; syncBusUi(); applyBus(); });
$("bloss").addEventListener("input", () => { bus.loss = +$("bloss").value; syncBusUi(); applyBus(); });
$("bdown").addEventListener("click", () => { bus.down = !bus.down; syncBusUi(); applyBus(); });

function setNode(b, { stalledNow, ms, text, warn }) {
  const dead = ms > TIMEOUT_MS;
  b.classList.toggle("stalled", stalledNow); b.classList.toggle("dead", dead || !!warn);
  b.setAttribute("aria-pressed", stalledNow);
  b.querySelector(".st").textContent = text;
  b.querySelector(".bar i").style.width = Math.min(100, (ms / TIMEOUT_MS) * 100) + "%";
}
function updateSwcs(o) {
  const sec = (ms) => (ms / 1000).toFixed(1);
  // ECU: ウォッチドッグ(VCU)の無応答判定 + ECU自身のフェイルセーフ
  ecuEls.forEach((b, k) => {
    const i = NODE_DEFS[k][1], ms = o[SILENT0 + i], fs = o[FS0 + k] | 0;
    let text = ms > TIMEOUT_MS ? `応答なし ${sec(ms)}s` : stalled[i] ? `停止中 ${sec(ms)}s` : "正常";
    if (fs & 2) text += "\n⚠ 自律EStop"; else if (fs & 1) text += "\n⚠ 入力0扱い";
    setNode(b, { stalledNow: stalled[i], ms, text, warn: fs });
  });
  const pms = o[SILENT0 + 3];
  setNode(plantEl, { stalledNow: stalled[3], ms: pms,
    text: pms > TIMEOUT_MS ? `応答なし ${sec(pms)}s` : stalled[3] ? `停止中 ${sec(pms)}s` : "正常" });
  setNode(vcuEl, { stalledNow: vcuStalled, ms: 0, text: vcuStalled ? "停止中（Safety/入力を送信しない）" : "正常",
    warn: vcuStalled && (o[FS0] | 0) & 2 });
}

function clearPath() { path.length = 0; pathDist = 0; trail.length = 0; }
$("clearTrail").addEventListener("click", clearPath);
// 5cm以上動いたときだけ点を追加（長時間でも点数が増えすぎない）
function addPath(x, y) {
  const p = path[path.length - 1];
  if (p) { const d = Math.hypot(x - p.x, y - p.y); if (d < 0.05) return; pathDist += d; }
  path.push({ x, y });
}

function setSlider(id, v) { $(id).value = v; $(id + "O").value = (+v).toFixed(2); }
for (const id of ["thr", "brk", "str"]) $(id).addEventListener("input", () => { $(id + "O").value = (+$(id).value).toFixed(2); });
$("spd").addEventListener("input", () => { speed = +$("spd").value; $("spdO").value = speed.toFixed(1) + "x"; });

function setEstop(on) {
  estop = on;
  $("estop").classList.toggle("on", on); $("estop").setAttribute("aria-pressed", on);
  $("estop").textContent = on ? "E-STOP 作動中（再押下で解除）" : "E-STOP（Space）";
}
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
  // steerCmd > 0 は左旋回 → ハンドルは反時計回り（canvasのrotateは時計回りが正なので符号反転）。表示角は右回り=+
  const steer = o[IDX.steerCmd], rot = -(steer / steerMax) * (450 * Math.PI / 180);
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

// 軌跡全体を枠に収めて描く（全体を自動フィット、等縮尺）。現在位置は丸で表示
function drawMap(o) {
  const c = $("map"), d = fit(c), g = c.getContext("2d");
  const W = c.width, H = c.height, pad = 16 * d;
  g.fillStyle = css("--bg"); g.fillRect(0, 0, W, H);
  g.strokeStyle = css("--line"); g.lineWidth = d; g.strokeRect(0.5, 0.5, W - 1, H - 1);
  const cur = { x: o[IDX.x], y: o[IDX.y] }, pts = path.concat([cur, { x: 0, y: 0 }]);
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  for (const p of pts) { x0 = Math.min(x0, p.x); x1 = Math.max(x1, p.x); y0 = Math.min(y0, p.y); y1 = Math.max(y1, p.y); }
  const span = Math.max(x1 - x0, y1 - y0, 0.5); // 最小0.5m幅
  const S = Math.min((W - 2 * pad) / span, (H - 2 * pad) / span);
  const ox = W / 2 - ((x0 + x1) / 2) * S, oy = H / 2 + ((y0 + y1) / 2) * S;
  const X = (x) => ox + x * S, Y = (y) => oy - y * S;
  g.fillStyle = css("--sub"); g.beginPath(); g.arc(X(0), Y(0), 4 * d, 0, 7); g.fill(); // スタート地点
  g.strokeStyle = css("--accent"); g.lineWidth = 2.5 * d; g.lineJoin = "round"; g.beginPath();
  path.forEach((p, i) => (i ? g.lineTo(X(p.x), Y(p.y)) : g.moveTo(X(p.x), Y(p.y))));
  g.lineTo(X(cur.x), Y(cur.y)); g.stroke();
  g.fillStyle = estop || o[IDX.state] === 2 ? css("--danger") : css("--car");
  g.beginPath(); g.arc(X(cur.x), Y(cur.y), 5 * d, 0, 7); g.fill();
  g.fillStyle = css("--sub"); g.font = `${11 * d}px system-ui`;
  g.fillText(`表示幅 ${span.toFixed(1)} m`, 8 * d, H - 8 * d);
  $("dist").textContent = pathDist.toFixed(2) + " m";
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
  line("v", css("--accent"), 0, vMax * 1.07);
  line("steer", css("--ok"), -steerMax * 1.12, steerMax * 1.12);
  g.font = `${11 * d}px system-ui`;
  g.fillStyle = css("--accent"); g.fillText(`v (0–${vMax} m/s)`, 8 * d, 14 * d);
  g.fillStyle = css("--ok"); g.fillText(`steer cmd (±${steerMax} rad, +=左)`, 110 * d, 14 * d);
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
  const sf = SAFETY[o[IDX.state]] || { cls: "degraded", ico: "?", lbl: "UNKNOWN", sub: "不明" };
  $("safety").className = "safety " + sf.cls;
  // 手動E-STOPでもウォッチドッグ由来のEStopでも、走行ビューに赤枠とバナーを出す
  const sysEstop = o[IDX.state] === 2;
  $("viewwrap").classList.toggle("estop", sysEstop || estop);
  $("estopBanner").textContent = estop ? "■ E-STOP 作動中" : "■ E-STOP（ウォッチドッグ検出）";
  $("safetyIco").textContent = sf.ico; $("safetyLbl").textContent = sf.lbl; $("safetySub").textContent = sf.sub;
  if (scenario) { setSlider("thr", o[IDX.thr]); setSlider("brk", o[IDX.brk]); setSlider("str", -o[IDX.str]); }
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
      trail.push({ x: o[IDX.x], y: o[IDX.y] }); addPath(o[IDX.x], o[IDX.y]);
      hist.push({ t: o[IDX.t], v: o[IDX.v], steer: o[IDX.steerCmd] });
    }
    if (trail.length > 6000) trail.splice(0, trail.length - 6000);
    while (hist.length && hist[0].t < hist[hist.length - 1].t - HISTORY_S - 1) hist.shift();
  }
  const o = out();
  drawView(o); drawWheel(o); drawChart(); drawMap(o); updateHud(o); updateSwcs(o);
}

// ---------- Config / NVM（Rust側の sdv_nvm_* / sdv_boot に対応）----------
const PRESETS = {
  sport: { engine: { max_accel_mps2: 4.0 }, vehicle: { max_speed_mps: 5.0 } },
  strict: { safety: { heartbeat_timeout_ms: 300 }, failsafe: { safety_timeout_ms: 50, driver_timeout_ms: 100 } },
  bad: { engine: { max_accel_mps2: 99 }, steering: { max_steer_angle_rad: 1.2 }, safety: { heartbeat_timeout_ms: 50 } },
};
const SRC_NAMES = ["Active（現行の設定）", "Backup（Activeが壊れていたため）", "既定値（NVMが空か両方破損）"];
let bootSource = 2;

function putText(str) {
  const bytes = new TextEncoder().encode(str);
  const p = wasm.sdv_alloc(bytes.length);
  new Uint8Array(mem.buffer, p, bytes.length).set(bytes);
  return [p, bytes.length];
}
function readBuf() { return new Uint8Array(mem.buffer, wasm.sdv_buf_ptr(), wasm.sdv_buf_len()).slice(); }
function bufText() { return new TextDecoder().decode(readBuf()); }

// ブラウザのlocalStorageを疑似NVMの記憶媒体にする（使えない環境ではメモリ上のみ）
const LS_KEY = (slot) => "sdv.nvm." + slot;
function persistNvm() {
  try {
    for (const slot of [0, 1]) {
      if (wasm.sdv_nvm_read(slot)) localStorage.setItem(LS_KEY(slot), btoa(String.fromCharCode(...readBuf())));
      else localStorage.removeItem(LS_KEY(slot));
    }
  } catch (_) { /* localStorage unavailable */ }
}
function restoreNvm() {
  try {
    for (const slot of [0, 1]) {
      const b64 = localStorage.getItem(LS_KEY(slot));
      if (!b64) continue;
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const p = wasm.sdv_alloc(bytes.length);
      new Uint8Array(mem.buffer, p, bytes.length).set(bytes);
      wasm.sdv_nvm_write(slot, p, bytes.length); wasm.sdv_free(p, bytes.length);
    }
  } catch (_) { /* ignore */ }
}

function showMsg(text, ok) { const m = $("cfgMsg"); m.textContent = text; m.className = "msg " + (ok ? "ok" : "err"); }
function slotText(name, st) { return `${name}: ` + (st === -1 ? "空" : st === -2 ? "破損" : `正常 (seq ${st})`); }
function refreshNvm() {
  const box = $("nvmStatus"); box.textContent = "";
  for (const [name, slot] of [["Active", 0], ["Backup", 1]]) {
    const st = wasm.sdv_nvm_slot_status(slot), sp = document.createElement("span");
    sp.textContent = slotText(name, st); if (st === -2) sp.className = "bad"; box.appendChild(sp);
  }
  const src = document.createElement("span"); src.className = "src"; src.textContent = "起動時の読み込み元: " + SRC_NAMES[bootSource]; box.appendChild(src);
}
// 実行中のConfigをUI（テキスト・グラフのスケール・ウォッチドッグの表示）へ反映
function applyConfigToUi() {
  wasm.sdv_config_json();
  const text = bufText(), cfg = JSON.parse(text);
  $("cfgText").value = text;
  TIMEOUT_MS = cfg.safety.heartbeat_timeout_ms;
  vMax = +cfg.vehicle.max_speed_mps.toFixed(1); steerMax = +cfg.steering.max_steer_angle_rad.toFixed(2);
}
function bootFromNvm() {
  bootSource = wasm.sdv_boot();
  applyConfigToUi(); setEstop(false); resetSim(); refreshNvm();
}

$("cfgSave").addEventListener("click", () => {
  const [p, n] = putText($("cfgText").value);
  const r = wasm.sdv_nvm_commit(p, n); wasm.sdv_free(p, n);
  if (r === 0) { persistNvm(); bootFromNvm(); showMsg("検証に通ったので保存し、再起動して適用しました。", true); }
  else if (r === 1) showMsg("拒否しました（何も保存していません）:\n" + bufText(), false);
  else showMsg("文字コードが不正です。", false);
});
$("cfgRollback").addEventListener("click", () => {
  if (wasm.sdv_nvm_rollback() === 0) { persistNvm(); bootFromNvm(); showMsg("ひとつ前のConfigに戻して再起動しました。", true); }
  else showMsg("戻せるバックアップがありません。", false);
});
$("nvmReboot").addEventListener("click", () => { bootFromNvm(); showMsg("NVMから読み込んで再起動しました。", true); });
for (const [id, slot, name] of [["corruptA", 0, "Active"], ["corruptB", 1, "Backup"]]) {
  $(id).addEventListener("click", () => {
    wasm.sdv_nvm_corrupt(slot); persistNvm(); refreshNvm();
    showMsg(`${name}を破損させました（データがあれば1バイト反転）。「再起動」で復旧動作を確認できます。`, true);
  });
}
$("nvmWipe").addEventListener("click", () => {
  wasm.sdv_nvm_erase(0); wasm.sdv_nvm_erase(1); persistNvm(); refreshNvm();
  showMsg("NVMを消去しました。次の再起動は既定値で起動します。", true);
});
for (const b of document.querySelectorAll(".preset")) {
  b.addEventListener("click", () => {
    const k = b.dataset.p;
    if (k === "default") { wasm.sdv_default_config_json(); $("cfgText").value = bufText(); }
    else $("cfgText").value = JSON.stringify(PRESETS[k], null, 2);
    showMsg("テキストを入れ替えました。「検証して保存」で反映します。", true);
  });
}

(async () => {
  try {
    const res = await fetch("sdv_core.wasm");
    const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
    wasm = instance.exports; mem = wasm.memory;
    restoreNvm();
    bootFromNvm();
    requestAnimationFrame(frame);
  } catch (e) {
    const el = $("err"); el.style.display = "block";
    el.textContent = "WASMの読み込みに失敗しました: " + e.message + "（ファイルを直接開かず、HTTP経由で表示してください）";
  }
})();
