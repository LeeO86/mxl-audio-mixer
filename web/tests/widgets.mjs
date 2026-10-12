// Framing check of the widgets (Appendix B.2) in headless Chrome: a page on an allowed origin frames each
// widget and gets widget-ready and widget-size; a page on a refused origin gets nothing (the CSP
// frame-ancestors blocks the frame). Screenshots of the framed widgets go to <out dir>.
//
// node web/tests/widgets.mjs <mixer base> <allowed parent base> <refused parent base> <out dir>
import { mkdirSync } from "node:fs";
import { chromium } from "playwright-core";

const [mixer, allowed, refused, out] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const pass = (what) => console.log(`[widgets.mjs] PASS: ${what}`);
const browser = await chromium.launch({ executablePath: process.env.CHROME || "/usr/bin/google-chrome", args: ["--no-sandbox"] });
try {
  const page = await (await browser.newContext({ viewport: { width: 700, height: 560 } })).newPage();
  const messages = async () => {
    const t = await page.title();
    return t === "waiting" ? [] : JSON.parse(t);
  };
  const widgets = [
    ["strip", "strip=vox&theme=dark"],
    ["fader-bank", "layout=Show&page=Talent&theme=light"],
    ["loudness", "signal=main&theme=dark"],
    ["monitor", "monitor=main&theme=dark"],
    ["talkback", "theme=dark"],
  ];
  for (const [id, query] of widgets) {
    const src = `${mixer}/widget/${id}?${query}`;
    await page.goto(`${allowed}/frame.html?src=${encodeURIComponent(src)}`);
    await page.waitForFunction(() => document.title.includes("widget-size"), null, { timeout: 8000 });
    const m = await messages();
    const size = m.find((x) => x.type === "widget-size");
    if (m[0]?.type !== "widget-ready" || !(size?.w > 0 && size?.h > 0)) throw new Error(`${id}: ${JSON.stringify(m)}`);
    await page.waitForTimeout(700);
    await page.screenshot({ path: `${out}/widget-${id}.png` });
    pass(`${id}: framed from an allowed origin, widget-ready and widget-size ${size.w}×${size.h}`);
  }
  await page.goto(`${refused}/frame.html?src=${encodeURIComponent(`${mixer}/widget/strip?strip=vox`)}`);
  await page.waitForTimeout(3000);
  const m = await messages();
  if (m.length) throw new Error(`a refused origin got ${JSON.stringify(m)}`);
  pass("a refused origin: the frame is blocked, no messages");
} finally {
  await browser.close();
}
