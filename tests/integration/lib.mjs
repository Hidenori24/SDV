// Shared by the integration runners: drives sdv_core.wasm through its C ABI and evaluates declarative checks.
import { readFileSync } from "node:fs";

// Layout of the output buffer (rust/sdv-core/src/lib.rs `OUT_LEN` doc).
export const SIGNALS = [
  "t", "x", "y", "yaw", "v", "yaw_rate", "wheel_omega", "throttle", "brake", "steer",
  "drive_accel_cmd", "brake_decel_cmd", "steer_angle_cmd", "estop", "state",
  "silent_engine_ms", "silent_brake_ms", "silent_steering_ms", "silent_plant_ms",
  "failsafe_engine", "failsafe_brake", "failsafe_steering",
  "stalled_engine", "stalled_brake", "stalled_steering", "stalled_plant", "stalled_vcu", "bus_down",
];

export async function load(wasmPath) {
  const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
  const w = instance.exports;
  const mem = () => w.memory.buffer;
  const text = (s) => {
    const b = new TextEncoder().encode(s);
    const p = w.sdv_alloc(b.length);
    new Uint8Array(mem(), p, b.length).set(b);
    return [p, b.length];
  };
  const bufText = () => new TextDecoder().decode(new Uint8Array(mem(), w.sdv_buf_ptr(), w.sdv_buf_len()));
  const withText = (s, f) => { const [p, n] = text(s); try { return f(p, n); } finally { w.sdv_free(p, n); } };

  /** Run a scenario (and optional partial config) for `seconds`; returns one row (Float32Array) per 10 ms tick. */
  function run(scenario, seconds, config) {
    w.sdv_nvm_erase(0); w.sdv_nvm_erase(1);
    if (config) {
      const r = withText(JSON.stringify(config), (p, n) => w.sdv_nvm_commit(p, n));
      if (r !== 0) throw new Error("config rejected: " + bufText());
    }
    w.sdv_boot();
    const r = withText(JSON.stringify(scenario), (p, n) => w.sdv_set_scenario(p, n));
    if (r !== 0) throw new Error("scenario rejected: " + bufText());
    w.sdv_use_scenario(1);
    w.sdv_reset();
    const rows = [];
    for (let i = 0; i < Math.round(seconds * 100); i++) {
      w.sdv_step(1);
      rows.push(new Float32Array(mem(), w.sdv_out_ptr(), w.sdv_out_len()).slice());
    }
    return rows;
  }
  return { run };
}

const EPS = 1e-4;
const HALF_TICK = 0.005; // times are f32 multiples of 10 ms: compare with half a tick of slack
const OPS = {
  "==": (a, v) => Math.abs(a - v) <= EPS,
  "!=": (a, v) => Math.abs(a - v) > EPS,
  "<": (a, v) => a < v, "<=": (a, v) => a <= v + EPS, ">": (a, v) => a > v, ">=": (a, v) => a >= v - EPS,
  approx: (a, v, c) => Math.abs(a - v) <= c.tol,
};

function sample(row, signal) {
  if (signal === "lateral_accel") return row[4] * row[5]; // v * yaw_rate
  const i = SIGNALS.indexOf(signal);
  if (i < 0) throw new Error(`unknown signal '${signal}'`);
  return row[i];
}
const holds = (row, c) => OPS[c.op](sample(row, c.signal), c.value, c);
const describe = (c) => `${c.signal} ${c.op} ${c.value}${c.op === "approx" ? ` (±${c.tol})` : ""}`;

/** Evaluate the checks of one case; returns the list of failures (empty = pass). */
export function evaluate(rows, checks) {
  const failures = [];
  const T = (r) => r[0];
  for (const c of checks) {
    if (!(c.op in OPS)) { failures.push(`unknown op '${c.op}' in ${JSON.stringify(c)}`); continue; }
    if (c.kind === "at") {
      const row = rows.find((r) => T(r) >= c.t - HALF_TICK);
      if (!row) failures.push(`at ${c.t}s: the run ended before this time`);
      else if (!holds(row, c)) failures.push(`at ${c.t}s: expected ${describe(c)}, got ${sample(row, c.signal)}`);
    } else if (c.kind === "always") {
      const bad = rows.find((r) => T(r) >= c.from - HALF_TICK && T(r) <= c.to + HALF_TICK && !holds(r, c));
      if (bad) failures.push(`always [${c.from}, ${c.to}]s: ${describe(c)} violated at t=${T(bad).toFixed(2)} (got ${sample(bad, c.signal)})`);
      if (!rows.some((r) => T(r) >= c.to - HALF_TICK)) failures.push(`always [${c.from}, ${c.to}]s: the run ended before ${c.to}s`);
    } else if (c.kind === "first") {
      const row = rows.find((r) => holds(r, c));
      if (!row) failures.push(`first: ${describe(c)} never became true`);
      else if (T(row) < c.between[0] - HALF_TICK || T(row) > c.between[1] + HALF_TICK)
        failures.push(`first: ${describe(c)} became true at t=${T(row).toFixed(2)}, expected within [${c.between}]`);
    } else failures.push(`unknown check kind '${c.kind}'`);
  }
  return failures;
}
