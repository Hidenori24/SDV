"use strict";

const $ = (id) => document.getElementById(id);
const initialImages = {
  A: { version: "1.0.0", gain: 0.8, curve: "linear" },
  B: { version: "0.9.0", gain: 0.8, curve: "linear" },
};
let images = cloneImages(initialImages);
let activeSlot = "A";
let failedSlot = null;
let lastComparison = null;
let updating = false;

function cloneImages(source) {
  return Object.fromEntries(Object.entries(source).map(([slot, image]) => [slot, { ...image }]));
}

function selectedMode() {
  return document.querySelector('input[name="variant"]:checked').value;
}

function nextImage(mode) {
  const current = images[activeSlot];
  const [major, minor] = current.version.split(".").map(Number);
  return {
    version: `${major}.${minor + 1}.0`,
    gain: mode === "constant" ? Number((current.gain + 0.2).toFixed(2)) : current.gain,
    curve: mode === "logic" ? (current.curve === "linear" ? "deadband" : "linear") : current.curve,
  };
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function steeringOutput(image, angle) {
  if (image.curve === "linear") return clamp(angle * image.gain, -25, 25);
  const deadband = Math.max(0, Math.abs(angle) - 3);
  return clamp(Math.sign(angle) * deadband * image.gain * 0.95, -25, 25);
}

function signed(value) {
  const rounded = Math.round(value * 10) / 10;
  return `${rounded > 0 ? "+" : ""}${rounded.toFixed(1)}°`;
}

function imageDescription(image) {
  return `v${image.version} · gain ${image.gain.toFixed(2)} · ${image.curve}`;
}

function renderSlots(candidate = nextImage(selectedMode()), trialSlot = null) {
  for (const slot of ["A", "B"]) {
    const card = $(`slot${slot}`);
    const isActive = slot === activeSlot;
    card.classList.toggle("active", isActive);
    card.classList.toggle("trial", slot === trialSlot);
    card.classList.toggle("failed", slot === failedSlot);
    $(`state${slot}`).textContent = isActive ? "active" : slot === trialSlot ? "trial" : slot === failedSlot ? "failed" : "fallback";
    $(`image${slot}`).textContent = imageDescription(images[slot]);
  }
  $("activeSummary").textContent = `active slot: ${activeSlot} · v${images[activeSlot].version}`;
  $("targetSlot").textContent = `書込先: Slot ${activeSlot === "A" ? "B" : "A"}`;
  const from = lastComparison?.baseline ?? images[activeSlot];
  const shownCandidate = lastComparison?.candidate ?? candidate;
  const mode = lastComparison?.mode ?? selectedMode();
  $("packageChange").textContent = mode === "constant"
    ? `Steering gain: ${from.gain.toFixed(2)} → ${shownCandidate.gain.toFixed(2)}`
    : `Response logic: ${from.curve} → ${shownCandidate.curve}`;
}

function chartPath(image, xToSvg, yToSvg) {
  const points = [];
  for (let angle = -30; angle <= 30; angle += 1) {
    const output = steeringOutput(image, angle);
    points.push(`${angle === -30 ? "M" : "L"}${xToSvg(angle).toFixed(1)},${yToSvg(output).toFixed(1)}`);
  }
  return points.join(" ");
}

function renderChart() {
  const mode = selectedMode();
  const active = lastComparison?.baseline ?? images[activeSlot];
  const candidate = lastComparison?.candidate ?? nextImage(mode);
  const angle = Number($("inputAngle").value);
  const oldOutput = steeringOutput(active, angle);
  const newOutput = steeringOutput(candidate, angle);
  const left = 52, right = 18, top = 18, bottom = 36;
  const width = 640 - left - right, height = 260 - top - bottom;
  const xToSvg = (x) => left + ((x + 30) / 60) * width;
  const yToSvg = (y) => top + ((28 - y) / 56) * height;
  const grid = [-20, -10, 0, 10, 20].map((tick) => {
    const y = yToSvg(tick).toFixed(1);
    return `<line x1="${left}" y1="${y}" x2="${640 - right}" y2="${y}" stroke="var(--line)"/><text x="${left - 8}" y="${Number(y) + 4}" fill="var(--sub)" text-anchor="end" font-size="11">${tick}</text>`;
  }).join("");
  const xLabels = [-30, -15, 0, 15, 30].map((tick) => {
    const x = xToSvg(tick).toFixed(1);
    return `<line x1="${x}" y1="${top}" x2="${x}" y2="${260 - bottom}" stroke="var(--line)"/><text x="${x}" y="${250}" fill="var(--sub)" text-anchor="middle" font-size="11">${tick}°</text>`;
  }).join("");
  $("responseChart").innerHTML = `${grid}${xLabels}
    <path d="${chartPath(active, xToSvg, yToSvg)}" fill="none" stroke="#8792a5" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="${chartPath(candidate, xToSvg, yToSvg)}" fill="none" stroke="var(--accent)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
    <circle cx="${xToSvg(angle).toFixed(1)}" cy="${yToSvg(oldOutput).toFixed(1)}" r="5" fill="#8792a5" stroke="var(--panel)" stroke-width="2"/>
    <circle cx="${xToSvg(angle).toFixed(1)}" cy="${yToSvg(newOutput).toFixed(1)}" r="5" fill="var(--accent)" stroke="var(--panel)" stroke-width="2"/>
    <text x="${left + width / 2}" y="258" fill="var(--sub)" text-anchor="middle" font-size="11">driver input</text>
    <text x="14" y="${top + height / 2}" fill="var(--sub)" text-anchor="middle" font-size="11" transform="rotate(-90 14 ${top + height / 2})">steering output</text>`;
  $("inputValue").textContent = `${angle > 0 ? "+" : ""}${angle}°`;
  $("oldOutput").textContent = signed(oldOutput);
  $("newOutput").textContent = signed(newOutput);
  $("outputDelta").textContent = signed(newOutput - oldOutput);
  $("responseChart").setAttribute("aria-label", `入力 ${angle}度。更新前 ${signed(oldOutput)}、更新案 ${signed(newOutput)}。横軸はdriver input、縦軸はsteering output。`);
  renderSlots(candidate);
}

function addLog(message) {
  const item = document.createElement("li");
  item.textContent = message;
  $("eventLog").append(item);
  $("eventLog").scrollTop = $("eventLog").scrollHeight;
}

function setProgress(percent, currentStep = -1, failedStep = -1) {
  $("progressFill").style.width = `${percent}%`;
  $("progress").setAttribute("aria-valuenow", String(percent));
  document.querySelectorAll(".flowstep").forEach((step, index) => {
    step.classList.toggle("done", index < currentStep || (failedStep >= 0 && index < failedStep) || (currentStep === -1 && percent === 100));
    step.classList.toggle("current", index === currentStep);
    step.classList.toggle("failed", index === failedStep);
  });
}

function setStatus(message, kind = "") {
  $("status").className = `status${kind ? ` ${kind}` : ""}`;
  $("status").textContent = message;
}

function setBusy(busy) {
  updating = busy;
  $("runUpdate").disabled = busy;
  $("resetDemo").disabled = busy;
  $("failHealth").disabled = busy;
  document.querySelectorAll('input[name="variant"]').forEach((input) => { input.disabled = busy; });
}

const pause = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));

async function runUpdate() {
  if (updating) return;
  setBusy(true);
  $("eventLog").replaceChildren();
  setProgress(0);
  lastComparison = null;
  renderChart();
  const oldActive = activeSlot;
  const baseline = { ...images[oldActive] };
  const targetSlot = oldActive === "A" ? "B" : "A";
  const mode = selectedMode();
  const candidate = nextImage(mode);
  const modeLabel = mode === "constant" ? "gain変更" : "steeringロジック変更";
  const steps = [
    ["署名検証: release keyでpackageを確認", 14],
    [`${modeLabel} packageをSlot ${targetSlot}へstaging`, 32],
    ["readback digestがmanifestと一致", 50],
    [`Slot ${targetSlot}をtrial起動（出力hold中）`, 67],
    ["Steering ECU healthを3 cycle確認", 86],
    [`trial metadataをcommitしてSlot ${targetSlot}をactiveに設定`, 100],
  ];

  try {
    for (let index = 0; index < steps.length; index += 1) {
      const [message, progress] = steps[index];
      setProgress(progress, index);
      setStatus(message);
      addLog(message);
      if (index === 1) {
        images[targetSlot] = candidate;
        failedSlot = null;
        renderSlots(candidate, targetSlot);
      }
      await pause(620);
      if (index === 4 && $("failHealth").checked) {
        failedSlot = targetSlot;
        lastComparison = { baseline, candidate: { ...candidate }, mode };
        setProgress(progress, -1, index);
        addLog("health check failed; trial metadata cleared");
        addLog(`rollback: Slot ${oldActive} remains active`);
        setStatus(`health check失敗。Slot ${oldActive}のactive imageを維持し、candidateをrollbackしました。`, "fail");
        renderChart();
        return;
      }
      if (index === 5) {
        activeSlot = targetSlot;
        failedSlot = null;
        lastComparison = { baseline, candidate: { ...candidate }, mode };
        renderChart();
      }
    }

    setProgress(100, -1);
    setStatus(`更新成功。Slot ${targetSlot} v${candidate.version}をactiveにcommitしました。旧slotはrollback用に残っています。`, "ok");
    renderChart();
  } finally {
    setBusy(false);
  }
}

function resetDemo() {
  if (updating) return;
  images = cloneImages(initialImages);
  activeSlot = "A";
  failedSlot = null;
  lastComparison = null;
  document.querySelector('input[name="variant"][value="constant"]').checked = true;
  $("failHealth").checked = false;
  $("inputAngle").value = 18;
  $("eventLog").replaceChildren();
  setProgress(0);
  setStatus("更新内容を選び、「FOTAを実行」を押してください。");
  renderChart();
}

document.querySelectorAll('input[name="variant"]').forEach((input) => input.addEventListener("change", () => {
  lastComparison = null;
  renderChart();
}));
$("inputAngle").addEventListener("input", renderChart);
$("runUpdate").addEventListener("click", runUpdate);
$("resetDemo").addEventListener("click", resetDemo);
renderChart();
