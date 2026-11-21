/** 请求头对象与多行文本互转。面板与 EPG 共用。 */

export type Header = { name: string; value: string };

function readString(item: object, key: string): string {
  const value = (item as Record<string, unknown>)[key];
  return typeof value === 'string' ? value.trim() : '';
}

/** Header 对象数组 → 多行文本。名称必须非空；值可为空。 */
export function headersToText(raw: unknown): string {
  if (!Array.isArray(raw)) return '';
  return raw
    .map((item) => {
      if (!item || typeof item !== 'object') return null;
      const name = readString(item, 'name');
      const value = readString(item, 'value');
      return name ? `${name}: ${value}` : null;
    })
    .filter((line): line is string => line !== null)
    .join('\n');
}

/** 多行文本 → Header 对象数组。名称必须非空；值可为空。 */
export function parseHeaders(text: string): Header[] {
  return text
    .split('\n')
    .map((line) => {
      const index = line.indexOf(':');
      if (index < 0) return null;
      const name = line.slice(0, index).trim();
      const value = line.slice(index + 1).trim();
      return name ? { name, value } : null;
    })
    .filter((item): item is Header => item !== null);
}
