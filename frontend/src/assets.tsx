/**
 * 视频素材与字幕管理分区。
 *
 * 接口：`/panel/api/{assets,subtitle}/{list,upload,remove,raw/:file}`、
 * `/panel/api/subtitle/translate{/status,/raw/:name}` 与
 * `/panel/api/assets/ffmpeg-status`。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Badge, PageHead } from './ui';

type Entry = {
  file: string;
  name: string;
  ext: string;
  bytes: number;
  mtime: number;
};

type Kind = 'assets' | 'subtitle';

const LABEL: Record<Kind, string> = {
  assets: '视频素材',
  subtitle: '字幕文件',
};

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 带会话头下载面板文件。 */
async function download(path: string, name: string): Promise<void> {
  const token = localStorage.getItem('tv_buddy_token') ?? '';
  const response = await fetch(`/panel/api${path}`, {
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  if (!response.ok) {
    throw new Error(`下载失败：${response.status}`);
  }
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

type TranslationInfo = {
  enabled: boolean;
  engine: string;
  targets: string[];
  cached: number;
};

/** 配置里指向 `/assets/<文件名>` 的地址集合（递归扫所有字符串）。 */
function referencedNames(config: unknown): Set<string> {
  const names = new Set<string>();
  const visit = (node: unknown): void => {
    if (typeof node === 'string') {
      const match = /\/assets\/([^/?#"]+)/.exec(node);
      if (match) names.add(decodeURIComponent(match[1]));
      return;
    }
    if (Array.isArray(node)) {
      node.forEach(visit);
      return;
    }
    if (node && typeof node === 'object') {
      Object.values(node as Record<string, unknown>).forEach(visit);
    }
  };
  visit(config);
  return names;
}

function Library({
  kind,
  accept,
  ffmpegReady,
}: {
  kind: Kind;
  accept: string;
  ffmpegReady: boolean;
}) {
  const [files, setFiles] = useState<Entry[]>([]);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [translation, setTranslation] = useState<TranslationInfo | null>(null);
  const [target, setTarget] = useState('');
  const [busy, setBusy] = useState('');
  const [referenced, setReferenced] = useState<Set<string>>(new Set());
  const [onlyUnused, setOnlyUnused] = useState(false);

  useEffect(() => {
    if (kind !== 'subtitle') {
      return;
    }
    void api<TranslationInfo>('GET', '/subtitle/translate/status')
      .then((info) => {
        setTranslation(info);
        setTarget(info.targets[0] ?? '');
      })
      .catch(() => setTranslation(null));
  }, [kind]);

  const load = useCallback(async () => {
    try {
      const data = await api<{ files: Entry[] }>('GET', `/${kind}/list`);
      setFiles(data.files);
      setError('');
      if (kind === 'assets') {
        const config = await api<unknown>('GET', '/config');
        setReferenced(referencedNames(config));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [kind]);

  useEffect(() => {
    void load();
  }, [load]);

  async function upload(input: FileList | null) {
    if (!input || input.length === 0) {
      return;
    }
    setMessage('');
    setError('');
    try {
      const items = await Promise.all(
        Array.from(input).map(async (file) => ({
          name: file.name,
          data: await readAsDataUrl(file),
        })),
      );
      const result = await api<{ added: Entry[]; failed: { name: string; error: string }[] }>(
        'POST',
        `/${kind}/upload`,
        { items },
      );
      setMessage(`已上传 ${result.added.length} 个，失败 ${result.failed.length} 个`);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function remove(file: string) {
    if (!window.confirm(`删除 ${file}？`)) {
      return;
    }
    try {
      await api('POST', `/${kind}/remove`, { file });
      await load();
      setMessage('已删除');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function copyLink(name: string) {
    const url = `${location.origin}/assets/${encodeURIComponent(name)}`;
    setMessage((await copyText(url)) ? `已复制 ${url}` : '复制失败，请手动复制');
  }

  async function translateFile(file: Entry) {
    if (!target) {
      return;
    }
    setBusy(file.file);
    setMessage('');
    setError('');
    try {
      const result = await api<{ download: string; cached: boolean }>(
        'POST',
        '/subtitle/translate',
        { file: file.file, target },
      );
      await download(result.download, `${file.name}.${target}.${file.ext}`);
      setMessage(`${file.file} → ${target}（${result.cached ? '复用缓存' : '新译'}）`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy('');
    }
  }

  const unused = files.filter((file) => !referenced.has(file.file));
  const shown = onlyUnused ? unused : files;

  return (
    <div className="card">
      <div className="row between">
        <h2>{LABEL[kind]}</h2>
        <div className="row">
          {kind === 'subtitle' && translation?.enabled && translation.targets.length > 1 && (
            <select value={target} onChange={(event) => setTarget(event.target.value)}>
              {translation.targets.map((code) => (
                <option key={code} value={code}>
                  {code}
                </option>
              ))}
            </select>
          )}
          {kind === 'assets' && (
            <label className="tag" style={{ cursor: 'pointer' }}>
              <input
                type="checkbox"
                checked={onlyUnused}
                onChange={(event) => setOnlyUnused(event.target.checked)}
              />
              只看未被引用
            </label>
          )}
          <label className="tag" style={{ cursor: 'pointer' }}>
            上传
            <input
              type="file"
              accept={accept}
              multiple
              style={{ display: 'none' }}
              onChange={(event) => void upload(event.target.files)}
            />
          </label>
        </div>
      </div>
      {kind === 'assets' && !ffmpegReady && (
        <p className="muted">未检测到 ffmpeg：转封装（remux/dash）保持 503 预热。</p>
      )}
      {kind === 'subtitle' && translation && !translation.enabled && (
        <p className="muted">字幕翻译未启用：请在配置中打开 [subtitle.translate].enabled。</p>
      )}
      {kind === 'subtitle' && translation?.enabled && translation.engine === 'none' && (
        <p className="muted">未配置 MTranServer 地址：翻译请求会返回 400。</p>
      )}
      {kind === 'assets' && (
        <p className="muted">
          未被引用的素材 {unused.length} 个（共 {files.length} 个）。判定标准：配置里没有任何地址指向
          `/assets/&lt;文件名&gt;`（本实现自定，目标未观测）。
        </p>
      )}
      <table className="table">
        <thead>
          <tr>
            <th>文件</th>
            <th className="right">大小</th>
            <th className="right">操作</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((file) => (
            <tr key={file.file}>
              <td className="mono ellipsis">
                {file.file}
                {kind === 'assets' && !referenced.has(file.file) && (
                  <Badge tone="muted">未被引用</Badge>
                )}
              </td>
              <td className="right">{formatBytes(file.bytes)}</td>
              <td className="right">
                <a
                  className="ghost"
                  href={`/panel/api/${kind}/raw/${encodeURIComponent(file.file)}`}
                  target="_blank"
                  rel="noreferrer"
                >
                  打开
                </a>
                {kind === 'assets' && (
                  <button className="icon-btn" onClick={() => void copyLink(file.file)}>
                    复制外链
                  </button>
                )}
                <button className="icon-btn" onClick={() => void remove(file.file)}>
                  删除
                </button>
                {kind === 'subtitle' &&
                  translation?.enabled &&
                  ['.srt', '.vtt'].includes(`.${file.ext}`) && (
                    <button
                      className="icon-btn"
                      disabled={busy === file.file}
                      onClick={() => void translateFile(file)}
                    >
                      {busy === file.file ? '翻译中' : '翻译'}
                    </button>
                  )}
              </td>
            </tr>
          ))}
          {shown.length === 0 && (
            <tr>
              <td colSpan={3} className="muted">
                暂无文件。
              </td>
            </tr>
          )}
        </tbody>
      </table>
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 远程素材转换：`POST /panel/api/assets/convert {url}`。 */
function ConvertCard({ ready }: { ready: boolean }) {
  const [url, setUrl] = useState('');
  const [result, setResult] = useState<{ local_path: string; bytes: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const run = async () => {
    setBusy(true);
    setError('');
    setResult(null);
    try {
      setResult(
        await api<{ local_path: string; bytes: number }>('POST', '/assets/convert', {
          url: url.trim(),
        }),
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <h2>远程转换</h2>
      <p className="muted">
        用 ffmpeg 把上游地址转成本地 HLS，并写入 `[assets].map`。需要 ffmpeg。
      </p>
      <div className="row">
        <input
          placeholder="http(s)://…/live.ts"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          style={{ flex: 1 }}
        />
        <button onClick={() => void run()} disabled={!ready || busy || url.trim() === ''}>
          {busy ? '转换中…' : '转换'}
        </button>
      </div>
      {!ready && <div className="msg err">未检测到 ffmpeg，无法转换。</div>}
      {result && (
        <div className="msg ok">
          已生成{' '}
          <a href={result.local_path} target="_blank" rel="noreferrer">
            {result.local_path}
          </a>{' '}
          （{result.bytes} 字节）
        </div>
      )}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 配置里一个占位跳转 URL（`GET /panel/api/assets/candidates`）。 */
type Candidate = {
  key: string;
  label: string;
  url: string;
  local_path: string;
  converted: boolean;
};

/**
 * 配置中的占位视频 URL。
 *
 * 目标的 5 个占位键常指向 MP4。部分播放器不显示 MP4，
 * 所以提供逐项与一键转本地 HLS。转换只写 `[assets].map`，不改配置里的跳转键。
 */
function CandidatesCard({ ready }: { ready: boolean }) {
  const [items, setItems] = useState<Candidate[]>([]);
  const [running, setRunning] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const body = await api<{ items?: Candidate[] }>('GET', '/assets/candidates');
      setItems(body.items ?? []);
      setError('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const convert = async (url: string) => {
    setRunning(url);
    setError('');
    try {
      await api('POST', '/assets/convert', { url });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setRunning('');
    }
  };

  /** 一键转换：逐个串行，跳过已转换的。 */
  const convertAll = async () => {
    const pending = items.filter((item) => !item.converted && item.url !== '');
    for (const item of pending) {
      await convert(item.url);
    }
  };

  const pending = items.filter((item) => !item.converted && item.url !== '');
  const busy = running !== '';

  return (
    <div className="card">
      <div className="row between">
        <h2>配置中的占位视频 URL</h2>
        <button disabled={!ready || busy || pending.length === 0} onClick={() => void convertAll()}>
          一键转换（{pending.length}）
        </button>
      </div>
      <p className="muted">
        这些地址常指向 MP4。部分播放器不显示 MP4，转成本地 HLS 后可见。转换结果写入{' '}
        <code className="mono">[assets].map</code>，不改动配置里的跳转键。
      </p>
      <table className="table">
        <thead>
          <tr>
            <th>用途</th>
            <th>地址</th>
            <th className="right">状态</th>
            <th className="right">操作</th>
          </tr>
        </thead>
        <tbody>
          {items.map((item) => (
            <tr key={item.key}>
              <td>{item.label}</td>
              <td className="mono" style={{ wordBreak: 'break-all' }}>
                {item.url === '' ? '（未设置）' : item.url}
              </td>
              <td className="right">
                {item.converted ? <Badge tone="ok">已转换</Badge> : <Badge tone="muted">未转换</Badge>}
              </td>
              <td className="right">
                <button
                  disabled={!ready || busy || item.url === ''}
                  onClick={() => void convert(item.url)}
                >
                  {running === item.url ? '转换中…' : item.converted ? '重新转换' : '转换为 HLS'}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {items.length === 0 && <p className="muted">未读取到占位 URL。</p>}
      {!ready && <div className="msg err">未检测到 ffmpeg，无法转换。</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 素材页：视频、字幕与 ffmpeg 状态。 */
export function AssetsPage() {
  const [ffmpeg, setFfmpeg] = useState<{ installed: boolean; version: string | null } | null>(null);

  useEffect(() => {
    void api<{ installed: boolean; version: string | null }>('GET', '/assets/ffmpeg-status')
      .then(setFfmpeg)
      .catch(() => setFfmpeg(null));
  }, []);

  return (
    <>
      <PageHead icon="assets" title="视频素材" sub="上传、转换与引用本地视频与字幕" />
      <div className="card">
        <div className="section-title">运行环境</div>
        <dl>
          <dt>ffmpeg</dt>
          <dd>
            {ffmpeg?.installed
              ? `已安装（${ffmpeg.version ?? '版本未知'}）`
              : '未安装（录制无法转 MKV；remux 不受影响）'}
          </dd>
        </dl>
      </div>
      <CandidatesCard ready={Boolean(ffmpeg?.installed)} />
      <Library kind="assets" accept="video/*" ffmpegReady={Boolean(ffmpeg?.installed)} />
      <ConvertCard ready={Boolean(ffmpeg?.installed)} />
      <Library kind="subtitle" accept=".srt,.ass,.ssa,.vtt" ffmpegReady={true} />
    </>
  );
}
