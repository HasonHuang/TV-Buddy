/**
 * 视频素材与字幕管理分区。
 *
 * 接口：`/panel/api/{assets,subtitle}/{list,upload,remove,raw/:file}`、
 * `/panel/api/subtitle/translate{/status,/raw/:name}` 与
 * `/panel/api/assets/ffmpeg-status`。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Badge, Confirm, Empty, PageHead, Select, Tip } from './ui';

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

/** 字幕目标语言的中文名。 */
const LANG_LABELS: Record<string, string> = {
  'zh-cn': '简体中文',
  'zh-tw': '繁體中文',
  ja: '日本語',
  ko: '한국어',
  en: '英语',
  es: '西班牙语',
  fr: '法语',
  de: '德语',
  ru: '俄语',
  he: '希伯来语',
  vi: '越南语',
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
  const [pendingRemove, setPendingRemove] = useState<string | null>(null);

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
        <h2>
          {LABEL[kind]}
          <Tip text="素材保存在缓存目录。文件名上传后不能改，频道按文件名查找。" />
        </h2>
        <div className="row">
          {kind === 'subtitle' && translation?.enabled && translation.targets.length > 1 && (
            <Select
              ariaLabel="翻译目标语言"
              title="翻译并生成对应语言的字幕文件"
              value={target}
              options={translation.targets.map((code) => ({
                value: code,
                label: LANG_LABELS[code] ?? code,
              }))}
              onChange={setTarget}
              compact
            />
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
          <label className="ghost" style={{ cursor: 'pointer' }} title="上传新文件">
            上传
            <input
              type="file"
              accept={accept}
              multiple
              style={{ display: 'none' }}
              onChange={(event) => void upload(event.target.files)}
            />
          </label>
          <Tip text="同名文件会被覆盖。字幕支持常见文本格式。视频转换需要 ffmpeg。" />
        </div>
      </div>
      {kind === 'assets' && !ffmpegReady && (
        <p className="muted">未检测到 ffmpeg：视频转换不可用。</p>
      )}
      {kind === 'subtitle' && translation && !translation.enabled && (
        <p className="muted">字幕翻译未启用。可以在「系统设置」的字幕设置里开启。</p>
      )}
      {kind === 'subtitle' && translation?.enabled && translation.engine === 'none' && (
        <p className="muted">还没有配置翻译服务地址，翻译会失败。</p>
      )}
      {kind === 'assets' && (
        <p className="muted">
          未被引用的素材 {unused.length} 个（共 {files.length} 个）。配置里没有任何地址指向的文件算作未被引用。
        </p>
      )}
      <div className="table-wrap">
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
                <td className="mono ellipsis" title={file.file}>
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
                    title="在新标签页打开原文件"
                  >
                    打开
                  </a>
                  {kind === 'assets' && (
                    <button
                      className="icon-btn"
                      title="复制这个素材的公网地址"
                      onClick={() => void copyLink(file.file)}
                    >
                      复制地址
                    </button>
                  )}
                  <button
                    className="chip danger-text"
                    title="删除文件；引用它的频道会失效"
                    onClick={() => setPendingRemove(file.file)}
                  >
                    删除
                  </button>
                  {kind === 'subtitle' &&
                    translation?.enabled &&
                    ['.srt', '.vtt'].includes(`.${file.ext}`) && (
                      <button
                        className="icon-btn"
                        title="按所选语言生成翻译字幕"
                        disabled={busy === file.file}
                        onClick={() => void translateFile(file)}
                      >
                        {busy === file.file ? '翻译中' : '翻译'}
                      </button>
                    )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {shown.length === 0 ? <Empty text="暂无文件。" /> : null}
      </div>
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}

      {pendingRemove ? (
        <Confirm
          title="删除文件？"
          text={`文件「${pendingRemove}」删除后，引用它的频道会失效。`}
          confirmText="删除文件"
          onCancel={() => setPendingRemove(null)}
          onConfirm={() => {
            const file = pendingRemove;
            setPendingRemove(null);
            void remove(file);
          }}
        />
      ) : null}
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
      <h2>远程视频转换</h2>
      <p className="muted">
        把远程视频转成本地可播放的格式。需要 ffmpeg。
      </p>
      <div className="row">
        <input
          aria-label="要转换的视频地址"
          title="转换结果会保存到本地素材目录"
          placeholder="http(s)://…/live.ts"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          style={{ flex: 1 }}
        />
        <button
          title="把远程视频转换成本地可播放的文件"
          onClick={() => void run()}
          disabled={!ready || busy || url.trim() === ''}
        >
          {busy ? '转换中…' : '转换'}
        </button>
        <Tip text="转换会占用 CPU 和磁盘。完成后使用本地地址播放。" />
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
        <h2>占位视频地址</h2>
        <button
          title="逐个转换所有未处理的占位地址"
          disabled={!ready || busy || pending.length === 0}
          onClick={() => void convertAll()}
        >
          一键转换（{pending.length}）
        </button>
      </div>
      <p className="muted">
        这些地址常指向 MP4。部分播放器不能播放 MP4，转成本地文件后可以显示。
        转换不会修改原配置。
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
                  className="ghost"
                  title={item.converted ? '重新转换这个地址' : '把这个地址转换成本地 HLS'}
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
