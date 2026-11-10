import { test as base } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const VISUAL_NOW = Date.parse('2026-11-01T12:00:00Z');
const fonts = join(__dirname, 'fixtures/fonts');
const css = `
  @font-face { font-family: NodeFlareVisual; src: url('/__visual-fonts/visual-regular.woff2'); font-weight: 100 500; }
  @font-face { font-family: NodeFlareVisual; src: url('/__visual-fonts/visual-bold.woff2'); font-weight: 600 900; }
  @font-face { font-family: NodeFlareFlags; src: url('/__visual-fonts/visual-flags.woff2'); }
  * { font-family: NodeFlareVisual, sans-serif !important; }
  .region-flag { font-family: NodeFlareFlags !important; }
`;

// Pin text rendering and dates as well as the browser revision. This fixture
// affects screenshot tests only; production keeps its normal system fonts.
export const test = base.extend<{ visualEnvironment: void }>({
  visualEnvironment: [async ({ page }, use) => {
    await page.clock.setFixedTime(new Date(VISUAL_NOW));
    await page.route('**/api/latency/**', route => route.fulfill({ json: { tasks: [], points: [] } }));
    await page.route('**/__visual-fonts/*', route => {
      const name = new URL(route.request().url()).pathname.split('/').at(-1)!;
      if (!['visual-regular.woff2', 'visual-bold.woff2', 'visual-flags.woff2'].includes(name)) return route.abort();
      return route.fulfill({ contentType: 'font/woff2', body: readFileSync(join(fonts, name)) });
    });
    await page.addInitScript(value => {
      const install = () => {
        const style = document.createElement('style'); style.textContent = value; document.head.append(style);
      };
      if (document.head) install(); else document.addEventListener('DOMContentLoaded', install, { once: true });
    }, css);
    await use();
  }, { auto: true }],
});
