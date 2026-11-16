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
  source_removed: '源已移除',
  source_disabled: '源已停用',
  source_changed: '源已变更',
  budget: '预算跳过',
  cancelled: '已取消',
  guard: '安全阀暂停',
  unknown: '未校验',
};

const ACTION_LABEL: Record<string, string> = {
  admitted: '准入',
  rejected: '拒绝',
  removed: '移出',
  kept: '保留',
  hidden: '下线',
  source_removed: '源移除',
  held: '暂停准入',
  removal_paused: '暂停移出',
  skipped: '跳过',
};

function actionTone(action: string): string {
  switch (action) {
    case 'admitted':
      return 'ok';
    case 'removed':
    case 'rejected':
      return 'danger';
    case 'hidden':
      return 'warn';
    case 'held':
    case 'removal_paused':
      return 'warn';
    default:
      return 'muted';
  }
}

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

/** 频道质量徽章：延迟、速率、分辨率、结果。无结果时不渲染。
 *
 * 传入 `onClick` 时整组徽章可点击，用于跳到质量结果页。
 */
export function QualityBadges({
  result,
  onClick,
}: {
  result?: QualityResult;
  onClick?: () => void;
}) {
  if (!result) return null;
  const hidden = result.hidden === true || result.action === 'hidden';
  const label = hidden ? '已隐藏' : REASON_LABEL[result.reason] ?? result.reason;
  const tone = hidden ? 'danger' : resultTone(result.reason);
  const resolution = result.resolution.split('x')[1] ?? '';
  const speed = result.speed_mib ?? 0;
  const tip = [
    `最近校验：${checkedText(result.checked_at)}`,
    `原因：${REASON_LABEL[result.reason] ?? result.reason}`,
    hidden ? '当前规则下会从订阅清单隐藏' : '',
    result.stale ? '结果已超过两个周期未更新' : '',
  ]
    .filter((line) => line !== '')
    .join('；');
  const content = (
    <>
      {result.delay_ms && result.delay_ms > 0 ? (
        <Badge tone="muted">{result.delay_ms}ms</Badge>
      ) : null}
      {speed > 0 ? <Badge tone="muted">{speed.toFixed(1)} MiB/s</Badge> : null}
      {resolution !== '' ? <Badge tone="muted">{resolution}p</Badge> : null}
      {result.action ? (
        <Badge tone={actionTone(result.action)}>
          {ACTION_LABEL[result.action] ?? result.action}
        </Badge>
      ) : null}
      <HoverTip text={tip}>
        <Badge tone={tone}>{label}</Badge>
      </HoverTip>
    </>
  );
  if (!onClick) return content;
  return (
    <button type="button" className="quality-jump" title="查看质量结果" onClick={onClick}>
      {content}
    </button>
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
    if (result.ok && !result.hidden && result.action !== 'hidden') valid += 1;
  }
  return { valid, tested };
}
