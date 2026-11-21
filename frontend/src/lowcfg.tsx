/**
 * 低配置优化：缓存状态页的推荐摘要卡。
 *
 * 依据 `docs/design/06-low-spec-optimization.md` §8。字段级设置表单已并入
 * `frontend/src/settings.tsx`——系统设置页统一编辑、统一保存整份配置。
 */

import { useState } from 'react';

import { copyText } from './api';
import { Tip } from './ui';

/** `GET /panel/api/cache/stats` 的 `recommend` 块。 */
export type Recommend = {
  ttl_secs: number;
  ttl_source: 'manual' | 'measured' | 'fallback';
  disk_cap_mb: number;
  cap_source: 'manual' | 'auto';
  limited_by:
    | 'manual'
    | 'working'
    | 'budget'
    | 'ceiling'
    | 'floor'
    | 'no_data'
    | 'space_unknown';
  channels: number;
  working_channels: number;
  working_mb: number | null;
  budget_mb: number | null;
  ceiling_mb: number | null;
  gc_last_run_secs_ago: number | null;
  groups: number;
  updated_secs_ago: number | null;
  disk_total_mb: number | null;
  disk_free_mb: number | null;
  segment_body_bytes: number;
  segment_index_bytes: number;
  segment_index_limit_bytes: number;
  segment_cache_bytes: number;
  segment_body_files: number;
  auto_tune: boolean;
};

const TTL_SOURCE_LABEL: Record<Recommend['ttl_source'], string> = {
  measured: '实测',
  manual: '手动配置',
  fallback: '回退值',
};

/** 自动缓存上限的限制来源。 */
export const LIMIT_LABEL: Record<Recommend['limited_by'], string> = {
  manual: '手动设置',
  working: '播放需要',
  budget: '可用空间',
  ceiling: '总容量 70%',
  floor: '下限',
  no_data: '无数据',
  space_unknown: '空间读取失败',
};

export function formatMB(mb: number | null | undefined): string {
  if (mb === null || mb === undefined) {
    return '—';
  }
  return `${mb} MB`;
}

export function formatAgo(seconds: number | null): string {
  if (seconds === null) return '暂无';
  if (seconds < 60) return `${seconds} 秒前`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`;
  return `${Math.floor(seconds / 3600)} 小时前`;
}

/** CDN 推荐摘要（缓存状态页）。`onEstimate` 提供立即估算入口。 */
export function RecommendCard({
  recommend,
  cdnEnabled,
  onEstimate,
  estimating = false,
}: {
  recommend: Recommend;
  /** `cdn.enabled`：关闭时提示 CDN 兜底未启用。推荐值仍可复制。 */
  cdnEnabled: boolean;
  onEstimate?: () => void;
  estimating?: boolean;
}) {
  const [copied, setCopied] = useState(false);

  const copyTtl = async () => {
    if (await copyText(String(recommend.ttl_secs))) {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    }
  };

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h2>
            CDN 推荐值
            <Tip text="根据最近的播放情况推算。把缓存时长填进 CDN 规则。" />
          </h2>
          <span className={`badge ${recommend.ttl_source === 'fallback' ? 'warn' : 'ok'}`}>
            {TTL_SOURCE_LABEL[recommend.ttl_source] ?? recommend.ttl_source}
          </span>
          {recommend.ttl_source === 'fallback' && (
            <span className="muted"> 尚未实测，先播放一个频道或点「立即估算」</span>
          )}
          {!cdnEnabled && <span className="muted"> CDN 兜底未启用</span>}
        </div>
        <div className="row">
          {onEstimate ? (
            <button
              className="ghost"
              title="立即采样一次工作集，更新推荐值"
              disabled={estimating}
              onClick={onEstimate}
            >
              {estimating ? '估算中…' : '立即估算'}
            </button>
          ) : null}
          <button
            className="ghost"
            title="复制推荐的缓存时长（秒）"
            onClick={() => void copyTtl()}
          >
            {copied ? '已复制' : '复制缓存时长'}
          </button>
        </div>
      </div>
      <div className="rec">
        <div className="rec-item">
          <div className="k">缓存时长</div>
          <div className="v">{recommend.ttl_secs}s</div>
        </div>
        <div className="rec-item">
          <div className="k">最近采样</div>
          <div className="v">{formatAgo(recommend.updated_secs_ago)}</div>
        </div>
      </div>
      <p className="muted">
        把缓存时长填进 CDN 面板的「强制缓存」规则。清单、面板和健康检查路径请设为不缓存。
      </p>
    </div>
  );
}
