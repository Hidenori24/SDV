// 紹介動画のフレームを撮る。
//   node capture.mjs <ページ> stills 4.5,24,33        … 指定した秒の静止画（still_<ページ>_<秒>.jpg）
//   node capture.mjs <ページ> scene <名前> <開始秒> <終了秒> … out/<名前>.mp4（30 fps）
// <ページ> は intro か sim-real。?gw=1280 のような指定は VIDEO_QUERY で渡す
// 前提: http://localhost:8123/ で tools/video/ を配信していること（render.sh が起動する）
import {chromium} from 'playwright';
import {spawn} from 'node:child_process';
import fs from 'node:fs';

const [pageName, mode, ...rest] = process.argv.slice(2);
const browser = await chromium.launch({
  executablePath: process.env.CHROME_PATH || undefined,
  args: ['--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--ignore-gpu-blocklist', '--no-sandbox'],
});
const page = await browser.newPage({viewport: {width: 1920, height: 1080}});
page.on('pageerror', e => console.error('PAGEERR', e.message));
await page.goto(`${process.env.VIDEO_BASE || 'http://localhost:8123'}/${pageName}.html?${process.env.VIDEO_QUERY || ''}`);
await page.waitForFunction('window.READY===true', null, {timeout: 120000});

// 3D（WebGL）と HUD（2D）を 1 枚に重ねて JPEG にする
const grab = async t => Buffer.from((await page.evaluate(t => {
  window.renderAt(t);
  const c = window._c || (window._c = Object.assign(document.createElement('canvas'), {width: 1920, height: 1080}));
  const x = c.getContext('2d');
  x.imageSmoothingQuality = 'high';
  x.drawImage(document.getElementById('gl'), 0, 0, 1920, 1080);
  x.drawImage(document.getElementById('hud'), 0, 0);
  return c.toDataURL('image/jpeg', 0.92);
}, t)).split(',')[1], 'base64');

if (mode === 'stills') {
  for (const t of rest[0].split(',')) {
    const t0 = Date.now();
    fs.writeFileSync(`still_${pageName}_${t}.jpg`, await grab(+t));
    console.log('still', t, Date.now() - t0, 'ms');
  }
} else if (mode === 'scene') {
  const [name, s, e] = rest;
  const f0 = Math.round(s * 30), f1 = Math.round(e * 30);
  fs.mkdirSync('out', {recursive: true});
  const tmp = `out/${name}.tmp.mp4`;  // 途中で止まった動画を完成品と取り違えないよう、成功時だけ改名する
  const ff = spawn('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'image2pipe', '-framerate', '30', '-c:v', 'mjpeg', '-i', '-',
    '-c:v', 'libx264', '-crf', '17', '-preset', 'veryfast', '-pix_fmt', 'yuv420p', '-r', '30', tmp], {stdio: ['pipe', 'inherit', 'inherit']});
  const done = new Promise(r => ff.on('exit', r));
  for (let f = f0; f < f1; f++) {
    const buf = await grab(f / 30);
    if (!ff.stdin.write(buf)) await new Promise(r => ff.stdin.once('drain', r));
    if ((f - f0) % 60 === 0) console.log(name, f - f0, '/', f1 - f0);
  }
  ff.stdin.end();
  const code = await done;
  if (code === 0) { fs.renameSync(tmp, `out/${name}.mp4`); console.log('OK', name); }
  else { console.error('ffmpeg failed', code); process.exitCode = 1; }
} else {
  console.error('usage: node capture.mjs <intro|sim-real> stills <t1,t2,...> | scene <name> <start> <end>');
  process.exitCode = 2;
}
await browser.close();
