/**
 * 低配置优化：推荐摘要卡与三张设置表单。
 *
 * 依据 `docs/design/06-low-spec-optimization.md` §8：
 * - 缓存状态页只读展示推荐 TTL / 磁盘上限与数据来源；
 * - 系统设置页编辑 `[cdn]`、`[cache.gc]`、`[cache.prefetch]`。
 *
 * `PUT /panel/api/config` 是整份替换，所以保存时先重新 GET，再只覆盖这三段。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { useToast } from './ui';

type Config = Record<string, unknown>;

type CdnCfg = {
  enabled: boolean;
  auto_tune: boolean;
  segment_max_age_secs: number;
  shared_cache_hits: boolean;
  dont_cache_empty: boolean;
  empty_status: number;
  public_base?: string | null;
};

type GcCfg = {
  enabled: boolean;
  max_disk_mb: number;
  max_age_secs: number;
  interval_secs: number;
  scan_limit: number;
  disk_budget_ratio: number;
  reserve_mb: number;
  min_mb: number;
};

type PrefetchCfg = {
  enabled: boolean;
  concurrency: number;
  timeout: number;
  prefetch_limit: number;
  skip_tail: number;
  mpd_readahead: number;
};

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

function NumberField({
  label,
  value,
  hint,
  step,
  onChange,
}: {
  label: string;
  value: number;
  hint?: string;
  step?: number;
  onChange: (next: number) => void;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      <input
        type="number"
        min={0}
        step={step ?? 1}
        value={String(value)}
        onChange={(event) => {
          const next = Number(event.target.value);
          onChange(Number.isFinite(next) ? next : 0);
        }}
      />
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

function CheckField({
  label,
  checked,
  hint,
  onChange,
}: {
  label: string;
  checked: boolean;
  hint?: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <div className="field">
      <label className="check">
        <input
          type="checkbox"
          checked={checked}
          onChange={(event) => onChange(event.target.checked)}
        />
        {label}
      </label>
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

/** 系统设置页的三张低配置表单。 */
export function LowSpecForms() {
  const [config, setConfig] = useState<Config | null>(null);
  const [recommend, setRecommend] = useState<Recommend | null>(null);
  const [busy, setBusy] = useState('');
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      const [next, stats] = await Promise.all([
        api<Config>('GET', '/config'),
        api<{ recommend: Recommend }>('GET', '/cache/stats'),
      ]);
      setConfig(next);
      setRecommend(stats.recommend);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const cache = (config?.cache ?? {}) as Record<string, unknown>;
  const cdn = config?.cdn as CdnCfg | undefined;
  const gc = cache.gc as GcCfg | undefined;
  const prefetch = cache.prefetch as PrefetchCfg | undefined;

  const setCdn = (patch: Partial<CdnCfg>) =>
    setConfig((prev) => (prev ? { ...prev, cdn: { ...(prev.cdn as CdnCfg), ...patch } } : prev));

  const setCache = (patch: Record<string, unknown>) =>
    setConfig((prev) => (prev ? { ...prev, cache: { ...cache, ...patch } } : prev));

  const save = async () => {
    if (!config) return;
    setBusy('save');
    try {
      // 只覆盖这三段：先取最新配置，避免把别处的改动回退掉。
      const fresh = await api<Config>('GET', '/config');
      const freshCache = { ...((fresh.cache ?? {}) as Record<string, unknown>) };
      freshCache.gc = gc;
      freshCache.prefetch = prefetch;
      const merged: Config = { ...fresh, cdn, cache: freshCache };
      await api('PUT', '/config', merged);
      setConfig(merged);
      const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
      setRecommend(stats.recommend);
      ok('低配置设置已保存');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const estimate = async () => {
    setBusy('estimate');
    try {
      const result = await api<{ groups: number; failed: { gid: string; cid: string }[] }>(
        'POST',
        '/cdn/estimate',
      );
      const failed = result.failed.length;
      ok(`估算完成：${result.groups} 个分组${failed ? `，失败 ${failed} 个` : ''}`);
      const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
      setRecommend(stats.recommend);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  if (!cdn || !gc || !prefetch) {
    return (
      <div className="card">
        <h2>低配置优化</h2>
        <p className="muted">读取配置中…</p>
      </div>
    );
  }

  const ttlHint =
    cdn.segment_max_age_secs === 0
      ? `自动（当前 ${recommend?.ttl_secs ?? '—'}s）`
      : '手动值，覆盖自动推荐';
  const capHint =
    gc.max_disk_mb === 0
      ? `自动（当前 ${recommend?.disk_cap_mb ?? '—'} MB）`
      : '手动值，覆盖自动推荐';

  return (
    <>
      <div className="card">
        <div className="row between">
          <div>
            <h2>CDN 缓存（[cdn]）</h2>
            <span className="muted">
              外部 CDN 侧的兜底；默认关闭，关闭时响应头与状态码保持原样。
            </span>
          </div>
          <div className="row">
            <button
              className="ghost"
              disabled={busy === 'estimate'}
              onClick={() => void estimate()}
            >
              {busy === 'estimate' ? '估算中…' : '立即估算'}
            </button>
            <button disabled={busy === 'save'} onClick={() => void save()}>
              {busy === 'save' ? '保存中…' : '保存'}
            </button>
          </div>
        </div>
        <CheckField
          label="启用 CDN 兜底"
          checked={cdn.enabled}
          hint="关闭时全部行为与旧版逐字节一致"
          onChange={(enabled) => setCdn({ enabled })}
        />
        <CheckField
          label="被动实测"
          checked={cdn.auto_tune}
          hint="从真实播放流量推算推荐值；关闭后只沿用上次落盘的估算"
          onChange={(auto_tune) => setCdn({ auto_tune })}
        />
        <div className="fields">
          <NumberField
            label="切片 max-age（秒，0=自动）"
            value={cdn.segment_max_age_secs}
            hint={ttlHint}
            onChange={(segment_max_age_secs) => setCdn({ segment_max_age_secs })}
          />
          <NumberField
            label="上游失败状态码"
            value={cdn.empty_status}
            hint="默认 502：让「200 + 空体」不被 CDN 缓存"
            onChange={(empty_status) => setCdn({ empty_status })}
          />
          <div className="field">
            <label>对外基址（可空）</label>
            <input
              value={cdn.public_base ?? ''}
              placeholder="https://cdn.example"
              onChange={(event) =>
                setCdn({ public_base: event.target.value.trim() === '' ? null : event.target.value })
              }
            />
            <div className="hint">CDN 不下发 X-Forwarded-Proto 时用</div>
          </div>
        </div>
        <CheckField
          label="命中路径也标 public"
          checked={cdn.shared_cache_hits}
          hint="某家 CDN 只能「遵循源站头」时需要"
          onChange={(shared_cache_hits) => setCdn({ shared_cache_hits })}
        />
        <CheckField
          label="空体加 no-store"
          checked={cdn.dont_cache_empty}
          hint="仅在 CDN 跟随源站头时有效；强制 TTL 时用上面的失败状态码"
          onChange={(dont_cache_empty) => setCdn({ dont_cache_empty })}
        />
      </div>

      <div className="card">
        <div className="row between">
          <h2>磁盘回收（[cache.gc]）</h2>
          <button disabled={busy === 'save'} onClick={() => void save()}>
            {busy === 'save' ? '保存中…' : '保存'}
          </button>
        </div>
        <CheckField
          label="启用磁盘回收"
          checked={gc.enabled}
          hint="切片正文只写不删；关闭后 1 GB 盘会被吃满"
          onChange={(enabled) => setCache({ gc: { ...gc, enabled } })}
        />
        <div className="fields">
          <NumberField
            label="占用上限（MB，0=自动）"
            value={gc.max_disk_mb}
            hint={capHint}
            onChange={(max_disk_mb) => setCache({ gc: { ...gc, max_disk_mb } })}
          />
          <NumberField
            label="最长保留（秒）"
            value={gc.max_age_secs}
            hint="超过即删，默认 1800"
            onChange={(max_age_secs) => setCache({ gc: { ...gc, max_age_secs } })}
          />
          <NumberField
            label="扫描周期（秒）"
            value={gc.interval_secs}
            hint="下限 30 秒"
            onChange={(interval_secs) => setCache({ gc: { ...gc, interval_secs } })}
          />
          <NumberField
            label="单轮扫描上限（个）"
            value={gc.scan_limit}
            hint="避免 1 vCPU 抖动，默认 500"
            onChange={(scan_limit) => setCache({ gc: { ...gc, scan_limit } })}
          />
          <NumberField
            label="可用空间占比"
            value={gc.disk_budget_ratio}
            step={0.05}
            hint="自动上限最多占可用空间的 80%"
            onChange={(disk_budget_ratio) => setCache({ gc: { ...gc, disk_budget_ratio } })}
          />
          <NumberField
            label="预留空间（MB）"
            value={gc.reserve_mb}
            hint="给系统、日志与其它缓存目录"
            onChange={(reserve_mb) => setCache({ gc: { ...gc, reserve_mb } })}
          />
          <NumberField
            label="自动下限（MB）"
            value={gc.min_mb}
            onChange={(min_mb) => setCache({ gc: { ...gc, min_mb } })}
          />
        </div>
      </div>

      <div className="card">
        <div className="row between">
          <h2>预取（[cache.prefetch]）</h2>
          <button disabled={busy === 'save'} onClick={() => void save()}>
            {busy === 'save' ? '保存中…' : '保存'}
          </button>
        </div>
        <p className="muted">
          预取只服务重复请求（重连、切台回退、多 CDN 扇出）；分组与频道可在「分组管理」里单独覆盖。
        </p>
        <CheckField
          label="启用预取"
          checked={prefetch.enabled}
          onChange={(enabled) => setCache({ prefetch: { ...prefetch, enabled } })}
        />
        <div className="fields">
          <NumberField
            label="并发数"
            value={prefetch.concurrency}
            hint="1 CPU 机器建议 2"
            onChange={(concurrency) => setCache({ prefetch: { ...prefetch, concurrency } })}
          />
          <NumberField
            label="单次上限（0=不限）"
            value={prefetch.prefetch_limit}
            onChange={(prefetch_limit) => setCache({ prefetch: { ...prefetch, prefetch_limit } })}
          />
          <NumberField
            label="超时（秒）"
            value={prefetch.timeout}
            onChange={(timeout) => setCache({ prefetch: { ...prefetch, timeout } })}
          />
          <NumberField
            label="跳过末尾片数"
            value={prefetch.skip_tail}
            hint="末尾切片可能还没写完，默认 1"
            onChange={(skip_tail) => setCache({ prefetch: { ...prefetch, skip_tail } })}
          />
        </div>
      </div>

      {node}
    </>
  );
}
