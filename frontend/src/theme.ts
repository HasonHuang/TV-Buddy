/** 明暗主题。
 *
 * 主题在 `<html>` 的 `data-theme` 上取 `light` 或 `dark`。入口 HTML 的
 * 内联脚本在首屏绘制前设好这个值，避免闪白：
 *
 * 1. 用户手动选过 → 用 `localStorage` 的值；
 * 2. 没选过 → 跟随系统。
 *
 * 手动切换后不再跟随系统。清掉 `tv_buddy_theme` 可恢复跟随。
 */

export type Theme = 'light' | 'dark';

const KEY = 'tv_buddy_theme';
const DARK_QUERY = '(prefers-color-scheme: dark)';
const THEME_COLOR: Record<Theme, string> = { light: '#f2f5fa', dark: '#0a0e16' };

function systemTheme(): Theme {
  return window.matchMedia(DARK_QUERY).matches ? 'dark' : 'light';
}

/** 当前生效的主题。首屏脚本已写入 `data-theme`，这里只读结果。 */
export function currentTheme(): Theme {
  return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
}

/** 套用主题并同步浏览器地址栏配色。 */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', THEME_COLOR[theme]);
}

/** 记住用户的选择并立即生效。 */
export function setTheme(theme: Theme): void {
  applyTheme(theme);
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // 隐私模式禁用存储；本次会话仍然生效。
  }
}

/** 在浅色与深色之间切换，返回切换后的主题。 */
export function toggleTheme(): Theme {
  const next: Theme = currentTheme() === 'dark' ? 'light' : 'dark';
  setTheme(next);
  return next;
}

/** 用户没手动选过时，跟随系统变化。
 *
 * 返回取消监听的函数。手动选过（存储里有值）就忽略系统变化。
 */
export function watchSystemTheme(onChange: (theme: Theme) => void): () => void {
  const media = window.matchMedia(DARK_QUERY);
  const listener = () => {
    try {
      if (localStorage.getItem(KEY) !== null) return;
    } catch {
      // 读不到存储时按未选过处理。
    }
    const theme = systemTheme();
    applyTheme(theme);
    onChange(theme);
  };
  media.addEventListener('change', listener);
  return () => media.removeEventListener('change', listener);
}
