/**
 * 台标库页；可独立挂载（`/logo`），也可嵌入管理面板。
 *
 * 公开浏览；登录后可上传、重命名、删除，并把台标应用到频道。
 */

import { useCallback, useEffect, useState } from 'react';
import { api, getToken } from './api';
import './style.css';

type Logo = {
  file: string;
  name: string;
  ext: string;
  bytes: number;
  url: string;
  /** 内容含动画块（只有 `.webp` 才可能为真）。 */
  animated?: boolean;
  /** `.orig/` 里有静态原图备份，可还原。 */
  has_original?: boolean;
};

type UploadResult = {
  added: Logo[];
  failed: { name: string; error: string }[];
};

/**
 * 动效：后端 `POST /panel/api/logo/animate` 的 6 种效果
 * （`src/logo_anim.rs` 的 match 分支）。`quality` 必须是**字符串**，
 * 且目标 v0.8.2 不读取它（帧时长与无损编码都固定），所以界面上只作为占位。
 */
const EFFECTS: { value: string; label: string }[] = [
  { value: 'diagonal', label: '斜向扫光' },
  { value: 'vertical', label: '纵向扫光' },
  { value: 'bounce', label: '弹跳' },
  { value: 'spot', label: '光斑' },
  { value: 'glow', label: '辉光' },
  { value: 'breathe', label: '呼吸' },
];

type AnimateReport = {
  frames: number;
  width: number;
  height: number;
  total_ms: number;
  bytes_before: number;
  bytes_after: number;
  dirty_ratio: number;
};

/** 浏览器侧转码参数（目标同：上载前用 canvas 重采样，服务端不转码）。 */
type UploadOpts = { maxEdge: string; quality: string; webp: boolean };

/** 按最长边等比缩放并（可选）转 WebP；返回 data URL 与扩展名。 */
async function transcode(
  file: File,
  opts: UploadOpts,
): Promise<{ name: string; ext: string; data: string }> {
  const maxEdge = Math.max(1, Math.min(4096, Number(opts.maxEdge) || 0));
  const quality = Math.max(1, Math.min(100, Number(opts.quality) || 80)) / 100;
  const bitmap = await createImageBitmap(file);
  const scale = Math.min(1, maxEdge / Math.max(bitmap.width, bitmap.height));
  const width = Math.max(1, Math.round(bitmap.width * scale));
  const height = Math.max(1, Math.round(bitmap.height * scale));

  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('浏览器不支持 canvas 转码');
  context.drawImage(bitmap, 0, 0, width, height);
  bitmap.close?.();

  // 不改尺寸也不转格式时，原样上载（避免无意义的重新压缩）。
  const unchanged = scale === 1 && !opts.webp;
  if (unchanged) {
    return { name: file.name, ext: extOf(file.name), data: await readAsDataUrl(file) };
  }

  const mime = opts.webp ? 'image/webp' : mimeOf(file.name);
  const blob = await new Promise<Blob | null>((resolve) =>
    canvas.toBlob(resolve, mime, quality),
  );
  if (!blob) throw new Error('转码失败：浏览器不支持该格式');
  const data = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
  const ext = opts.webp ? 'webp' : extOf(file.name);
  const base = file.name.replace(/\.[^.]+$/, '');
  return { name: `${base}.${ext}`, ext, data };
}

function extOf(name: string): string {
  return name.includes('.') ? name.split('.').pop()!.toLowerCase() : 'png';
}

function mimeOf(name: string): string {
  const ext = extOf(name);
  return ext === 'jpg' || ext === 'jpeg' ? 'image/jpeg' : `image/${ext}`;
}

type Missing = { gid: string; cid: string; name: string };

type Report = {
  channels: number;
  missing: number;
  broken: number;
  unused: number;
  missing_channels: Missing[];
  unused_logos: string[];
};

type Suggestion = {
  gid: string;
  cid: string;
  name: string;
  suggest: { file: string; score: number }[];
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function channelKey(channel: Missing): string {
  return `${channel.gid}/${channel.cid}`;
}

export function LogosPage() {
  const [logos, setLogos] = useState<Logo[]>([]);
  const [report, setReport] = useState<Report | null>(null);
  const [suggestions, setSuggestions] = useState<Record<string, string>>({});
  const [picked, setPicked] = useState<Record<string, string>>({});
  const [message, setMessage] = useState('');
  const [authed] = useState(() => Boolean(getToken()));
  const [opts, setOpts] = useState<UploadOpts>({ maxEdge: '512', quality: '80', webp: true });
  const [animating, setAnimating] = useState<{ file: string; effect: string } | null>(null);
  const [animReport, setAnimReport] = useState<AnimateReport | null>(null);

  const load = useCallback(async () => {
    const response = await fetch('/logo/api/list');
    if (!response.ok) {
      setMessage(`读取台标库失败：HTTP ${response.status}`);
      return;
    }
    const data = (await response.json()) as { logos?: Logo[] };
    setLogos(data.logos ?? []);
    if (!getToken()) {
      return;
    }
    const nextReport = await api<Report>('GET', '/logo/match-report');
    setReport(nextReport);
    const nextSuggest = await api<{ channels: Suggestion[] }>('POST', '/logo/suggest', {
      threshold: 0.4,
    });
    const map: Record<string, string> = {};
    for (const item of nextSuggest.channels) {
      const first = item.suggest[0];
      if (first) {
        map[`${item.gid}/${item.cid}`] = first.file;
      }
    }
    setSuggestions(map);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function upload(files: FileList | null) {
    if (!files || files.length === 0) {
      return;
    }
    try {
      const items = await Promise.all(Array.from(files).map((file) => transcode(file, opts)));
      const result = await api<UploadResult>('POST', '/logo/upload', { items });
      const failed = result.failed.map((item) => `${item.name}: ${item.error}`).join('；');
      setMessage(
        `已上传 ${result.added.length} 个，失败 ${result.failed.length} 个${failed ? `（${failed}）` : ''}`,
      );
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  /**
   * 生成动效（`POST /panel/api/logo/animate`）。
   * 服务端读 `[logo].max_size` 缩放，输出固定 10 帧无损动画 WebP 并覆盖原图；
   * 原图先进 `.orig/`，可用「还原」回退。
   */
  async function generate(file: string) {
    if (!animating || animating.file !== file) return;
    try {
      const report = await api<AnimateReport & { ok: boolean }>('POST', '/logo/animate', {
        file,
        effect: animating.effect,
        // 目标要求字符串；该值当前不被读取（帧时长与编码固定）。
        quality: opts.quality,
      });
      setAnimReport(report);
      setAnimating(null);
      setMessage(
        `${file}：${report.frames} 帧 ${report.width}×${report.height}，${report.total_ms} ms，` +
          `${formatBytes(report.bytes_before)} → ${formatBytes(report.bytes_after)}`,
      );
      await load();
    } catch (error) {
      setMessage(`动效生成失败：${(error as Error).message}`);
    }
  }

  /** 用 `.orig/` 备份还原静态原图。 */
  async function restore(file: string) {
    try {
      await api('POST', '/logo/restore', { file });
      setMessage(`已还原 ${file} 的静态原图`);
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function rename(file: string, current: string) {
    const name = window.prompt('新名称（扩展名不变）', current);
    if (!name) {
      return;
    }
    try {
      await api('POST', '/logo/rename', { file, name });
      setMessage('已重命名');
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function remove(file: string) {
    if (!window.confirm(`删除 ${file}？引用该文件的频道会变成坏引用。`)) {
      return;
    }
    try {
      await api('POST', '/logo/delete', { file });
      await load();
      setMessage('已删除');
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function apply(channel: Missing) {
    const key = channelKey(channel);
    const file = picked[key] ?? suggestions[key];
    if (!file) {
      setMessage('请选择台标文件');
      return;
    }
    try {
      await api('POST', '/logo/apply', { gid: channel.gid, cid: channel.cid, file });
      setMessage(`已把 ${file} 应用到 ${channel.name}`);
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  return (
    <>
      <div className="card">
        <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
          <h1>台标库</h1>
          {authed && (
            <label className="tag" style={{ cursor: 'pointer' }}>
              上传台标
              <input
                type="file"
                accept="image/*"
                multiple
                style={{ display: 'none' }}
                onChange={(event) => void upload(event.target.files)}
              />
            </label>
          )}
        </div>
        <p className="muted">
          共 {logos.length} 个台标。公开地址形如 <code className="mono">/logo/&lt;文件名&gt;</code>。
        </p>
        {authed && (
          <div className="row">
            <label>
              最大边
              <input
                type="number"
                min={16}
                max={2048}
                style={{ width: 90 }}
                value={opts.maxEdge}
                onChange={(event) => setOpts({ ...opts, maxEdge: event.target.value })}
              />
            </label>
            <label>
              质量
              <input
                type="number"
                min={1}
                max={100}
                style={{ width: 80 }}
                value={opts.quality}
                onChange={(event) => setOpts({ ...opts, quality: event.target.value })}
              />
            </label>
            <label>
              <input
                type="checkbox"
                checked={opts.webp}
                onChange={(event) => setOpts({ ...opts, webp: event.target.checked })}
              />
              转 WebP
            </label>
            <span className="muted">
              上载前在浏览器里按最长边等比缩放并转码（目标同：服务端上载不转码）。
            </span>
          </div>
        )}
        {report && (
          <p className="muted">
            频道 {report.channels} 个；缺台标 {report.missing} 个；坏引用 {report.broken} 个；未使用{' '}
            {report.unused} 个。
          </p>
        )}
        {message && <p className="msg">{message}</p>}
      </div>

      <div className="card" style={{ marginTop: 16 }}>
        {logos.length === 0 && <p className="muted">台标库为空。</p>}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(150px, 1fr))',
            gap: 12,
          }}
        >
          {logos.map((logo) => (
            <div key={logo.file} style={{ textAlign: 'center' }}>
              <img
                src={logo.url}
                alt={logo.name}
                style={{ maxWidth: '100%', maxHeight: 80, objectFit: 'contain' }}
              />
              <div className="mono ellipsis" style={{ fontSize: 12 }}>
                {logo.file}
              </div>
              <div className="muted" style={{ fontSize: 12 }}>
                {formatBytes(logo.bytes)}
                {logo.animated ? ' · 动效' : ''}
              </div>
              {authed && (
                <div className="row" style={{ justifyContent: 'center' }}>
                  <button className="ghost" onClick={() => void rename(logo.file, logo.name)}>
                    重命名
                  </button>
                  <button
                    className="ghost"
                    onClick={() => {
                      setAnimReport(null);
                      setAnimating({ file: logo.file, effect: EFFECTS[0].value });
                    }}
                  >
                    动效
                  </button>
                  {logo.has_original && (
                    <button className="ghost" onClick={() => void restore(logo.file)}>
                      还原
                    </button>
                  )}
                  <button className="icon-btn" onClick={() => void remove(logo.file)}>
                    删除
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      </div>

      {animating && (
        <div className="card" style={{ marginTop: 16 }}>
          <h2>生成动效</h2>
          <p className="muted">
            {animating.file}：输出固定 10 帧无损动画 WebP（前 9 帧 80 ms、末帧 1500 ms），
            覆盖原图前会先备份到 <code className="mono">.orig/</code>。
          </p>
          <div className="row">
            <label>
              效果
              <select
                value={animating.effect}
                onChange={(event) => setAnimating({ ...animating, effect: event.target.value })}
              >
                {EFFECTS.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}（{item.value}）
                  </option>
                ))}
              </select>
            </label>
            <button onClick={() => void generate(animating.file)}>生成</button>
            <button className="ghost" onClick={() => setAnimating(null)}>
              取消
            </button>
          </div>
          {animReport && (
            <p className="muted">
              上次结果：{animReport.frames} 帧 {animReport.width}×{animReport.height}，
              {animReport.total_ms} ms，脏像素 {animReport.dirty_ratio}%
            </p>
          )}
        </div>
      )}

      {authed && report && report.missing_channels.length > 0 && (
        <div className="card" style={{ marginTop: 16 }}>
          <h2>缺台标的频道</h2>
          <table className="table">
            <thead>
              <tr>
                <th>分组</th>
                <th>频道</th>
                <th>台标</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {report.missing_channels.slice(0, 100).map((channel) => {
                const key = channelKey(channel);
                return (
                  <tr key={key}>
                    <td className="mono">{channel.gid}</td>
                    <td>{channel.name}</td>
                    <td>
                      <select
                        value={picked[key] ?? suggestions[key] ?? ''}
                        onChange={(event) =>
                          setPicked({ ...picked, [key]: event.target.value })
                        }
                      >
                        <option value="">（选择文件）</option>
                        {logos.map((logo) => (
                          <option key={logo.file} value={logo.file}>
                            {logo.file}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td className="right">
                      <button className="ghost" onClick={() => void apply(channel)}>
                        应用
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
