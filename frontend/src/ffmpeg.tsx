/** ffmpeg 状态与面板安装。 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api, getRole } from './api';
import { formatBytes } from './format';
import { Badge, Confirm, Loading } from './ui';

type FfmpegJob = {
  phase: string;
  downloaded: number;
  total: number | null;
  error: string | null;
  started_at: number;
  finished_at: number | null;
};

type FfmpegStatus = {
  installed: boolean;
  version: string | null;
  source: 'managed' | 'system' | null;
  path: string | null;
  managed_installed: boolean;
  system_installed: boolean;
  ffprobe_installed: boolean;
  ffprobe_version: string | null;
  ffprobe_source: 'managed' | 'system' | null;
  ffprobe_path: string;
  ffprobe_managed_installed: boolean;
  job: FfmpegJob | null;
};

function sourceLabel(source: 'managed' | 'system' | null | undefined): string {
  if (source === 'managed') return '面板安装';
  if (source === 'system') return '系统';
  return '未知';
}

const PHASE_LABEL: Record<string, string> = {
  downloading: '下载中',
  verifying: '校验中',
  installing: '启用中',
  done: '完成',
  failed: '失败',
};

const ACTIVE = new Set(['downloading', 'verifying', 'installing']);

/** 读取 ffmpeg 状态，并在安装任务运行期间每秒刷新。 */
export function useFfmpeg() {
  const [status, setStatus] = useState<FfmpegStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [watching, setWatching] = useState(false);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await api<FfmpegStatus>('GET', '/assets/ffmpeg-status');
      if (mounted.current) {
        setStatus(next);
        setError('');
        if (!next.job || !ACTIVE.has(next.job.phase)) setWatching(false);
      }
    } catch (err) {
      if (mounted.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (mounted.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => {
      mounted.current = false;
    };
  }, [refresh]);

  const active = status?.job ? ACTIVE.has(status.job.phase) : false;
  useEffect(() => {
    if (!watching && !active) return;
    const timer = window.setInterval(() => {
      void refresh();
    }, 1000);
    return () => window.clearInterval(timer);
  }, [watching, active, refresh]);

  const run = useCallback(
    async (path: string, watch = false) => {
      setBusy(true);
      setError('');
      if (watch) setWatching(true);
      try {
        await api('POST', path);
        await refresh();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        if (watch) setWatching(false);
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const install = useCallback(() => run('/tools/ffmpeg/install', true), [run]);
  const remove = useCallback(() => run('/tools/ffmpeg/remove'), [run]);

  return { status, loading, busy, error, install, remove, refresh };
}

/** ffmpeg 状态卡：来源、版本、安装与删除。 */
export function FfmpegCard({ state }: { state: ReturnType<typeof useFfmpeg> }) {
  const { status, loading, busy, error, install, remove } = state;
  const [confirming, setConfirming] = useState(false);
  const admin = getRole() === 'admin';
  const job = status?.job ?? null;
  const active = job ? ACTIVE.has(job.phase) : false;
  const progress =
    job && job.total ? Math.min(100, Math.round((job.downloaded / job.total) * 100)) : null;

  return (
    <div className="card">
      <div className="section-title">ffmpeg / ffprobe</div>
      {loading ? (
        <Loading compact text="读取 ffmpeg 状态…" />
      ) : (
        <>
      <dl>
        <dt>ffmpeg</dt>
        <dd>
          {status?.installed ? (
            <Badge tone="ok">已安装</Badge>
          ) : (
            <Badge tone="warn">未安装</Badge>
          )}
          {status?.version ? (
            <span className="muted mono version"> {status.version}</span>
          ) : null}
          <span className="muted"> · 来源：{sourceLabel(status?.source)}</span>
        </dd>
        <dt>ffprobe</dt>
        <dd>
          {status?.ffprobe_installed ? (
            <Badge tone="ok">已安装</Badge>
          ) : (
            <Badge tone="warn">未安装</Badge>
          )}
          {status?.ffprobe_version ? (
            <span className="muted mono version"> {status.ffprobe_version}</span>
          ) : null}
          <span className="muted"> · 来源：{sourceLabel(status?.ffprobe_source)}</span>
        </dd>
        {status?.installed && status.path ? (
          <>
            <dt>ffmpeg 路径</dt>
            <dd>
              <code>{status.path}</code>
            </dd>
          </>
        ) : null}
        {status?.ffprobe_installed ? (
          <>
            <dt>ffprobe 路径</dt>
            <dd>
              <code>{status.ffprobe_path}</code>
            </dd>
          </>
        ) : null}
      </dl>

      {!status?.installed || !status?.ffprobe_installed ? (
        <p className="muted">
          {!status?.installed && !status?.ffprobe_installed
            ? 'ffmpeg 与 ffprobe 都未安装：DASH 录制只能保留 TS，素材转换与分辨率识别不可用；remux 不受影响。'
            : !status?.installed
              ? 'ffmpeg 未安装：DASH 录制只能保留 TS，素材转换不可用；分辨率识别仍可用；remux 不受影响。'
              : 'ffprobe 未安装：分辨率识别不可用；素材转换与 remux 不受影响。'}
        </p>
      ) : null}
      {(status?.source === 'managed' && !status.installed) ||
      (status?.ffprobe_source === 'managed' && !status.ffprobe_installed) ? (
        <div className="msg err">面板安装的版本不可用，请重新安装或删除。</div>
      ) : null}
      {error ? <div className="msg err">{error}</div> : null}
      {job && active ? (
        <p className="muted">
          {PHASE_LABEL[job.phase] ?? job.phase} · {formatBytes(job.downloaded)}
          {progress !== null ? `（${progress}%）` : ''}
        </p>
      ) : null}
      {job?.phase === 'failed' ? <div className="msg err">{job.error ?? '安装失败'}</div> : null}

      {admin ? (
        <div className="row">
          {!status?.installed || !status?.ffprobe_installed ? (
            <button disabled={busy || active} onClick={() => void install()}>
              {job?.phase === 'failed' ? '重试' : '安装 ffmpeg / ffprobe'}
            </button>
          ) : (
            <button className="ghost" disabled={busy || active} onClick={() => void install()}>
              重新安装
            </button>
          )}
          {status?.managed_installed ||
          status?.source === 'managed' ||
          status?.ffprobe_managed_installed ||
          status?.ffprobe_source === 'managed' ? (
            <button
              className="danger"
              disabled={busy || active}
              onClick={() => setConfirming(true)}
            >
              删除面板版本
            </button>
          ) : null}
        </div>
      ) : null}

      <p className="muted">来源：eugeneware/ffmpeg-static（GPLv3）</p>
        </>
      )}

      {confirming ? (
        <Confirm
          title="删除面板安装的 ffmpeg / ffprobe？"
          text="只删除数据目录里的 ffmpeg 与 ffprobe。系统安装的版本不受影响。"
          busy={busy}
          onCancel={() => setConfirming(false)}
          onConfirm={() => {
            setConfirming(false);
            void remove();
          }}
        />
      ) : null}
    </div>
  );
}
