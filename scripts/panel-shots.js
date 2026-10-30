#!/usr/bin/env node
// 面板逐屏截图：登录后依次截 13 个一级分区。
//
// 用途：P3-6 视觉对拍。见 `docs/tests/runs/2026-10-29.md`。
// 依赖：`npm i playwright`，以及本机 Chrome。
//
// 用法：node scripts/panel-shots.js <输出目录> [基址]
// 默认基址 http://127.0.0.1:19898。
// 账号取自环境变量 PANEL_USER / PANEL_PASS，默认 admin/admin。
// 首次安装的随机口令在 <cache>/admin_initial_credentials.txt。

const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const OUT = process.argv[2];
const BASE = process.argv[3] || 'http://127.0.0.1:19898';

if (!OUT) {
  console.error('用法：node scripts/panel-shots.js <输出目录> [基址]');
  process.exit(2);
}

// 导航项 → 参考图名。顺序与侧栏一致。
const SECTIONS = [
  ['概览', 'p01-dashboard'],
  ['缓存状态', 'p08-cache'],
  ['分组管理', 'p02-groups'],
  ['系统设置', 'p12-settings'],
  ['安全管控', 'p10-security'],
  ['频道热度', 'p11-stats'],
  ['定时录制', 'p06-schedule'],
  ['节目指南', 'p04-epg'],
  ['台标管理', 'p05-logos'],
  ['订阅聚合', 'p03-subs'],
  ['回看管理', 'p07-livestream'],
  ['视频素材', 'p13-assets'],
  ['运行日志', 'p09-logs'],
];

(async () => {
  fs.mkdirSync(OUT, { recursive: true });

  const browser = await chromium.launch({ channel: 'chrome' });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    deviceScaleFactor: 1,
  });
  const report = {};

  await page.goto(`${BASE}/panel`, { waitUntil: 'networkidle' });
  await page.waitForTimeout(600);
  await page.screenshot({ path: path.join(OUT, 'p00-login.png') });
  report.viewport = await page.evaluate(() => [innerWidth, innerHeight, devicePixelRatio]);

  await page.fill('#user', process.env.PANEL_USER || 'admin');
  await page.fill('#pass', process.env.PANEL_PASS || 'admin');
  await page.click('button[type=submit]');
  await page.waitForSelector('.sidebar .nav-item', { timeout: 15000 });
  await page.waitForTimeout(800);

  for (const [label, slug] of SECTIONS) {
    await page.evaluate((name) => {
      const btn = [...document.querySelectorAll('.sidebar .nav-item')].find(
        (b) => b.textContent.trim() === name,
      );
      if (btn) btn.click();
    }, label);
    await page.waitForTimeout(1000);
    await page.evaluate(() => window.scrollTo(0, 0));
    await page.waitForTimeout(200);

    report[slug] = await page.evaluate(() => ({
      title: document.querySelector('.page-head .title')?.textContent.trim() ?? null,
      sub: document.querySelector('.page-head .sub')?.textContent.trim() ?? null,
      actions: [...(document.querySelector('.page-head .actions')?.children ?? [])].map((e) =>
        e.textContent.trim(),
      ),
    }));

    await page.screenshot({ path: path.join(OUT, `${slug}.png`) });
    await page.screenshot({ path: path.join(OUT, `${slug}-full.png`), fullPage: true });
  }

  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
  await browser.close();
  console.log(`截图完成：${OUT}`);
})();
