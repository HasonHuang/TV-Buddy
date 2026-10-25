/**
 * 低配置优化：缓存状态页的推荐摘要卡（只读）。
 *
 * 依据 `docs/design/06-low-spec-optimization.md` §8。字段级设置表单已并入
 * `frontend/src/settings.tsx`——系统设置页统一编辑、统一保存整份配置。
 */

import { useState } from 'react';

import { copyText } from './api';


/** `GET /panel/api/cache/stats` 的 `recommend` 块。 */
export type Recommend = {
  ttl_secs: number;
  disk_cap_mb: number;
  source: string;
  ttl_manual: boolean;
  disk_manual: boolean;
  groups: number;
  updated_secs_ago: number | null;
  working_mb: number | null;
  disk_total_mb: number | null;
  disk_free_mb: number | null;
  seg_files: number;
  seg_bytes: number;
  gc_enabled: boolean;
  auto_tune: boolean;
};

const SOURCE_LABEL: Record<string, string> = {
  measured: '实测',
  manual: '手动配置',
  fallback: '回退值',
};

function formatMB(mb: number | null | undefined): string {
  if (mb === null || mb === undefined) {
    return '—';
  }
  return `${mb} MB`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatAgo(seconds: number | null): string {
  if (seconds === null) return '暂无';
  if (seconds < 60) return `${seconds} 秒前`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`;
  return `${Math.floor(seconds / 3600)} 小时前`;
}

/** 推荐摘要（缓存状态页，只读）。 */
export function RecommendCard({ recommend }: { recommend: Recommend }) {
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
          <h2>CDN 推荐值</h2>
          <span className={`badge ${recommend.source === 'fallback' ? 'warn' : 'ok'}`}>
            {SOURCE_LABEL[recommend.source] ?? recommend.source}
          </span>
          {recommend.source === 'fallback' && (
            <span className="muted"> 尚未实测，先播放一个频道或点「立即估算」</span>
          )}
        </div>
        <button className="ghost" onClick={() => void copyTtl()}>
          {copied ? '已复制' : '复制 TTL'}
        </button>
      </div>
      <div className="rec">
        <div className="rec-item">
          <div className="k">切片 TTL</div>
          <div className="v">{recommend.ttl_secs}s</div>
        </div>
        <div className="rec-item">
          <div className="k">磁盘上限</div>
          <div className="v">{formatMB(recommend.disk_cap_mb)}</div>
        </div>
        <div className="rec-item">
          <div className="k">实测工作集</div>
          <div className="v">{formatMB(recommend.working_mb)}</div>
        </div>
        <div className="rec-item">
          <div className="k">覆盖分组</div>
          <div className="v">{recommend.groups}</div>
        </div>
        <div className="rec-item">
          <div className="k">最近采样</div>
          <div className="v">{formatAgo(recommend.updated_secs_ago)}</div>
        </div>
        <div className="rec-item">
          <div className="k">切片占用</div>
          <div className="v">{formatBytes(recommend.seg_bytes)}</div>
        </div>
      </div>
      <dl>
        <dt>磁盘</dt>
        <dd>
          可用 {formatMB(recommend.disk_free_mb)} / 总 {formatMB(recommend.disk_total_mb)}
        </dd>
        <dt>被动实测</dt>
        <dd>{recommend.auto_tune ? '已开启' : '已关闭（沿用上次估算）'}</dd>
        <dt>磁盘回收</dt>
        <dd>{recommend.gc_enabled ? '已开启' : '已关闭（切片不会自动删除）'}</dd>
      </dl>
      <p className="muted">
        把 TTL 填进 CDN 面板的「强制 TTL」规则；切片路径用规则 B 缓存，清单与
        `/panel*`、`/health` 走 BYPASS。
      </p>
    </div>
  );
}

