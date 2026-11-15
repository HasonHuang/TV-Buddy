/** 质量校验徽章。用于分组页频道行与分组标题。
 *
 * 数据来自 Task 6 的 `GET /panel/api/quality/results`。健康探测的状态点
 * 保持原样，这里只叠加质量结果，不替代它。
 */

import type { QualityReason, QualityResult } from './api';
import { Badge, HoverTip } from './ui';

const REASON_LABEL: Record<QualityReason, string> = {
  pass: '合格',
  retained: '已保留',
  unreachable: '不可达',
  speed_low: '速率低',
  resolution_low: '分辨率低',
  resolution_high: '分辨率高',
  delay_high: '延迟高',
  supply: '兜底保留',
  probe_error: '探测错误',
  unknown: '未校验',
};

function resultTone(reason: QualityReason): string {
  switch (reason) {
    case 'pass':
      return 'ok';
    case 'retained':
    case 'supply':
      return 'info';
    case 'unreachable':
    case 'speed_low':
    case 'resolution_low':
    case 'resolution_high':
    case 'delay_high':
      return 'warn';
    default:
      return 'muted';
  }
}

function checkedText(epoch: number): string {
  if (!Number.isFinite(epoch) || epoch <= 0) return '未知时间';
  return new Date(epoch * 1000).toLocaleString('zh-CN', { hour12: false });
}

/** 频道质量徽章：延迟、速率、分辨率、结果。无结果时不渲染。 */
export function QualityBadges({ result }: { result?: QualityResult }) {
  if (!result) return null;
  const label = result.hidden ? '已隐藏' : REASON_LABEL[result.reason] ?? result.reason;
  const tone = result.hidden ? 'danger' : resultTone(result.reason);
  const resolution = result.height > 0 ? `${result.height}p` : '';
  const tip = [
    `最近校验：${checkedText(result.checked_at)}`,
    `原因：${REASON_LABEL[result.reason] ?? result.reason}`,
    result.hidden ? '当前规则下会从订阅清单隐藏' : '',
    result.stale ? '结果已超过两个周期未更新' : '',
  ]
    .filter((line) => line !== '')
    .join('；');
  return (
    <>
      {result.delay_ms > 0 ? <Badge tone="muted">{result.delay_ms}ms</Badge> : null}
      {result.speed_mib > 0 ? (
        <Badge tone="muted">{result.speed_mib.toFixed(1)} MiB/s</Badge>
      ) : null}
      {resolution !== '' ? <Badge tone="muted">{resolution}</Badge> : null}
      <HoverTip text={tip}>
        <Badge tone={tone}>{label}</Badge>
      </HoverTip>
    </>
  );
}

/** 分组标题的合格统计。只统计已有结果的频道。 */
export function qualityCounts(
  channels: { id: string }[],
  results: Record<string, QualityResult>,
  gid: string,
): { valid: number; tested: number } {
  let valid = 0;
  let tested = 0;
  for (const channel of channels) {
    const result = results[`${gid}/${channel.id}`];
    if (!result) continue;
    tested += 1;
    if (result.ok && !result.hidden) valid += 1;
  }
  return { valid, tested };
}
