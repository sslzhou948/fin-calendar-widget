// 效果图渲染脚本：在无头 Chromium 里把挂件 UI 叠在示意壁纸上截图。
// 用法：node scripts/screenshot.mjs [--side right|left] [--frosted on|off]
// 产出：docs/screenshots/*.png（渲染效果图，非真实桌面实拍）
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import puppeteer from 'puppeteer-core';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const SRC = join(ROOT, 'src');
const SHOT_DIR = join(ROOT, 'docs', 'screenshots');
mkdirSync(SHOT_DIR, { recursive: true });

// Chromium 路径：CI（GitHub Actions windows-latest）经 CHROME_PATH 环境变量覆盖，
// 本地默认沙盒内嵌 chromium。
const CHROME_PATH = process.env.CHROME_PATH || '/opt/meta-chromium/chrome';

const args = process.argv.slice(2);
const opt = (k, d) => { const i = args.indexOf(k); return i >= 0 ? args[i + 1] : d; };

let wallpaperCss;
try {
  const b64 = readFileSync(join(SHOT_DIR, 'assets', 'wallpaper-dark.jpg')).toString('base64');
  wallpaperCss = `url(data:image/jpeg;base64,${b64}) center/cover no-repeat`;
} catch {
  // CI/仓库内无二进制壁纸资源时，用程序化深色渐变代替（效果等价，不影响挂件本身验证）
  wallpaperCss = `linear-gradient(135deg,#0f172a 0%,#1e1b4b 45%,#0a0e1a 100%)`;
}
const widgetHtml = readFileSync(join(SRC, 'index.html'), 'utf8');
const widgetCss = readFileSync(join(SRC, 'styles.css'), 'utf8');
const widgetJs = readFileSync(join(SRC, 'app.js'), 'utf8');
const sampleData = JSON.parse(readFileSync(join(SRC, 'sample-data.json'), 'utf8'));

const bodyInner = (widgetHtml.match(/<body[^>]*>([\s\S]*?)<\/body>/i) || [,''])[1];

const icons = ['此电脑','回收站','Google Chrome','微信','Edge','文件夹','VS Code','终端'];
const iconHtml = icons.map(n =>
  `<div class="dicon"><div class="dglyph"></div><span>${n}</span></div>`).join('');

function composite({ side = 'right', frosted = true }) {
  const slotSide = side === 'right' ? 'right:28px;' : 'left:28px;';
  return `<!DOCTYPE html><html><head><meta charset="utf-8"><style>
    html,body{margin:0;padding:0;width:1920px;height:1080px;overflow:hidden}
    #desktop{position:relative;width:1920px;height:1080px;
      background:${wallpaperCss} #0a0e1a;}
    #icons{position:absolute;left:26px;top:24px;display:flex;flex-direction:column;gap:26px}
    .dicon{width:86px;text-align:center}
    .dglyph{width:52px;height:52px;margin:0 auto 6px;border-radius:12px;
      background:linear-gradient(135deg,rgba(255,255,255,.28),rgba(255,255,255,.08));
      border:1px solid rgba(255,255,255,.25)}
    .dicon span{font:12px/1.4 "PingFang SC","Microsoft YaHei",sans-serif;color:#fff;
      text-shadow:0 1px 3px rgba(0,0,0,.8)}
    #wslot{position:absolute;top:28px;bottom:28px;width:800px;${slotSide}}
    /* 挂件自身样式（作用域尽量隔离） */
  </style><style>${widgetCss}</style>
  <style>
    /* 桌面合成覆盖：保证挂件面板填满槽位 */
    #wslot #panel{height:100% !important;max-height:100% !important}
    ${frosted ? '' : '#wslot #panel{backdrop-filter:none !important;-webkit-backdrop-filter:none !important;background:rgba(10,12,20,.82) !important}'}
  </style></head>
  <body><div id="desktop"><div id="icons">${iconHtml}</div><div id="wslot">${bodyInner}</div></div>
  <script>
    window.__SAMPLE_DATA__ = ${JSON.stringify(sampleData)};
    window.__FLAG_DIR__ = ${JSON.stringify('file://' + SRC + '/flags/')};
    const __origFetch = window.fetch.bind(window);
    window.fetch = (u, ...r) => String(u).includes('sample-data.json')
      ? Promise.resolve(new Response(JSON.stringify(window.__SAMPLE_DATA__), {headers:{'Content-Type':'application/json'}}))
      : __origFetch(u, ...r);
  </script>
  <script>${widgetJs}</script>
  </body></html>`;
}

async function shot(name, html, w = 1920, h = 1080) {
  const browser = await puppeteer.launch({
    executablePath: CHROME_PATH, headless: 'new',
    args: ['--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--hide-scrollbars',
           '--force-device-scale-factor=1','--font-render-hinting=none'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: w, height: h });
  const file = join(SHOT_DIR, '_tmp_composite.html');
  writeFileSync(file, html);
  await page.goto('file://' + file, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1200));
  const out = join(SHOT_DIR, name);
  await page.screenshot({ path: out });
  await browser.close();
  console.log('saved', out);
}

// 设置页独立截图
async function shotSettings() {
  const html = readFileSync(join(SRC, 'settings.html'), 'utf8')
    .replace('</head>', `<style>html,body{margin:0}body{display:flex;justify-content:center;padding:40px;
      background:${wallpaperCss} #0a0e1a}</style></head>`);
  const browser = await puppeteer.launch({
    executablePath: CHROME_PATH, headless: 'new',
    args: ['--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--hide-scrollbars',
           '--force-device-scale-factor=1'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 860 });
  const file = join(SHOT_DIR, '_tmp_settings.html');
  let withAssets = html
    .replace(/<link[^>]*href=["']styles\.css["'][^>]*>/, `<style>${widgetCss}</style>`);
  const js = readFileSync(join(SRC, 'settings.js'), 'utf8');
  withAssets = withAssets.replace(/<script[^>]*src=["']settings\.js["'][^>]*><\/script>/, `<script>${js}</script>`);
  writeFileSync(file, withAssets);
  await page.goto('file://' + file, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 800));
  const out = join(SHOT_DIR, 'widget-settings.png');
  await page.screenshot({ path: out });
  await browser.close();
  console.log('saved', out);
}

await shot('widget-right-frosted.png', composite({ side: 'right', frosted: true }));
await shot('widget-left-nofrosted.png', composite({ side: 'left', frosted: false }));
await shotSettings();
