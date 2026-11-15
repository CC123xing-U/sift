// External Playwright keeps browser tooling out of the shipped application.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const output = path.resolve(process.env.UI_EVIDENCE_DIR || '../.work/ui-evidence');
fs.mkdirSync(output, { recursive: true });

(async () => {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 760 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(process.env.UI_BASE_URL || 'http://127.0.0.1:5177/');
    await page.waitForTimeout(3500);
    assert.equal(await page.locator('.topbar').evaluate(e => getComputedStyle(e).paddingLeft), '16px');
    const search = page.getByRole('textbox', { name: '搜索', exact: true });
    await search.fill('report');
    assert.equal(await search.evaluate(e => getComputedStyle(e).outlineStyle), 'none');
    await page.locator('.row').first().waitFor();
    assert.ok(await page.locator('.row').count() < 50);

    const sort = page.getByLabel('结果排序');
    await sort.selectOption('size:desc');
    await page.getByRole('button', { name: '大小，降序', exact: true }).click();
    assert.equal(await sort.inputValue(), 'size:asc');
    await sort.selectOption('score:desc');
    await page.getByRole('button', { name: '图片', exact: true }).click();
    assert.match(await search.inputValue(), /ext:jpg/);
    await page.getByRole('button', { name: '全部', exact: true }).click();
    assert.equal(await search.inputValue(), 'report');
    await page.waitForTimeout(800);

    for (const theme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme: theme });
      await page.waitForTimeout(300);
      const selected = await page.locator('.row.sel').evaluate(e => getComputedStyle(e).backgroundColor);
      assert.equal(selected, theme === 'light' ? 'rgb(224, 237, 247)' : 'rgb(47, 62, 69)');
      await page.locator('.app-window').screenshot({ path: path.join(output, `sift-ui-${theme}.png`) });
    }

    await page.locator('.index-status > summary').click();
    assert.equal(await page.locator('.index-status').getAttribute('open'), '');
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('.index-status').getAttribute('open'), null);
    assert.equal(await search.inputValue(), 'report');
    await page.locator('.row').first().click({ button: 'right' });
    await page.getByRole('menu').waitFor();
    await page.keyboard.press('Escape');
    assert.equal(await page.getByRole('menu').count(), 0);
    assert.equal(await search.inputValue(), 'report');

    await page.getByRole('button', { name: '设置', exact: true }).click();
    assert.equal(await page.locator('.settings-head').evaluate(e => e.getBoundingClientRect().height), 44);
    assert.ok(await page.getByRole('switch', { name: '模糊匹配', exact: true }).isEnabled());
    assert.ok(await page.getByRole('switch', { name: /开机自启/ }).isDisabled());
    await page.locator('.advanced-settings > summary').click();
    assert.ok(await page.getByRole('button', { name: /评分规则编辑器/ }).isDisabled());
    assert.equal(await page.getByRole('switch', { name: /评分规则/ }).count(), 0);
    await page.locator('.settings-body').evaluate(e => e.scrollTop = 0);
    await page.locator('.app-window').screenshot({ path: path.join(output, 'sift-ui-settings.png') });
    await page.keyboard.press('Escape');
    await page.locator('#sift-input').waitFor();

    await page.setViewportSize({ width: 480, height: 600 });
    await page.waitForTimeout(300);
    assert.ok(await page.evaluate(() => document.body.scrollWidth <= innerWidth));
    await page.locator('.app-window').screenshot({ path: path.join(output, 'sift-ui-narrow.png') });
    // Layout simulation only: no native IPC, hotkey or window lifecycle verification.
    await page.evaluate(() => document.documentElement.dataset.runtime = 'tauri');
    for (const size of [{ width: 760, height: 480 }, { width: 480, height: 320 }]) {
      await page.setViewportSize(size);
      await page.waitForTimeout(250);
      const bounds = await page.locator('.statusbar').boundingBox();
      assert.ok(bounds.y + bounds.height <= size.height);
      assert.ok(await page.locator('.row').count() < 50);
      assert.ok(await page.evaluate(() => document.body.scrollWidth <= innerWidth));
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await page.locator('.row').first().evaluate(e => getComputedStyle(e).transitionDuration), '1e-05s');
    await search.fill('report |');
    await page.locator('.errorbar').waitFor();
    const error = await page.locator('.errorbar').boundingBox();
    const filters = await page.locator('.filterbar').boundingBox();
    assert.ok(error.y >= filters.y + filters.height);
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ result: 'PASS', errors, screenshots: output }, null, 2));
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
