/** 面板共用的数值格式化。 */

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

/** 字节数转可读文本。最大到 TB。 */
export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B';
  let size = value;
  let index = 0;
  while (size >= 1024 && index < BYTE_UNITS.length - 1) {
    size /= 1024;
    index += 1;
  }
  // 不足 1 单位时保留一位小数，纵轴刻度不会都取整成同一个数。
  if (index === 0 && !Number.isInteger(size)) {
    return `${size.toFixed(1)} ${BYTE_UNITS[0]}`;
  }
  return `${size >= 100 || index === 0 ? Math.round(size) : size.toFixed(2)} ${
    BYTE_UNITS[index]
  }`;
}

/** 秒数转「N 天 M 小时」或「N 小时 M 分钟」。 */
export function formatUptime(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (days > 0) return `${days} 天 ${hours} 小时`;
  if (hours > 0) return `${hours} 小时 ${minutes} 分钟`;
  return `${minutes} 分钟`;
}

/** 地址显示：去掉 IPv4 映射前缀，便于阅读。 */
export function displayIp(value: string | null | undefined): string {
  if (!value) return 'unknown';
  return value.startsWith('::ffff:') ? value.slice(7) : value;
}
