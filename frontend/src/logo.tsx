/**
 * 台标库页。嵌入管理面板。
 *
 * 列表、上传、重命名、删除与应用都走面板会话。
 */

import { useCallback, useEffect, useState } from 'react';
import { api, getToken } from './api';
import { Confirm, Empty, Modal, NumberBox, PageHead, Select, Tip } from './ui';
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
  /** 批量操作进行中。为真时禁用批量与上传入口。 */
  const [busy, setBusy] = useState(false);
  const [pendingAction, setPendingAction] = useState<
    { kind: 'animate-all' } | { kind: 'restore-all' } | { kind: 'remove'; file: string } | null
  >(null);
  const [renaming, setRenaming] = useState<{ file: string; name: string } | null>(null);

  const load = useCallback(async () => {
    const data = await api<{ logos?: Logo[] }>('GET', '/logo/list');
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

  /**
   * 批量动效：给所有静态 `.webp` 台标生成动画。
   * 串行调用，避免同时压满 CPU。原图先备份到 `.orig/`。
   */
  async function animateAll() {
    const targets = logos.filter((item) => item.ext === 'webp' && item.animated !== true);
    if (targets.length === 0) {
      setMessage('没有需要生成动效的台标：只处理 .webp，且跳过已有动效的');
      return;
    }
    setBusy(true);
    let done = 0;
    const failed: string[] = [];
    for (const item of targets) {
      try {
        await api('POST', '/logo/animate', {
          file: item.file,
          effect: EFFECTS[0].value,
          quality: opts.quality,
        });
        done += 1;
      } catch (error) {
        failed.push(`${item.file}: ${(error as Error).message}`);
      }
      setMessage(`动效生成中：${done + failed.length}/${targets.length}`);
    }
    setBusy(false);
    setMessage(
      `动效完成 ${done} 个${failed.length > 0 ? `，失败 ${failed.length} 个（${failed.join('；')}）` : ''}`,
    );
    await load();
  }

  /** 批量还原：用 `.orig/` 备份覆盖所有带备份的台标。 */
  async function restoreAll() {
    const targets = logos.filter((item) => item.has_original === true);
    if (targets.length === 0) {
      setMessage('没有可还原的台标');
      return;
    }
    setBusy(true);
    let done = 0;
    const failed: string[] = [];
    for (const item of targets) {
      try {
        await api('POST', '/logo/restore', { file: item.file });
        done += 1;
      } catch (error) {
        failed.push(`${item.file}: ${(error as Error).message}`);
      }
      setMessage(`还原中：${done + failed.length}/${targets.length}`);
    }
    setBusy(false);
    setMessage(
      `还原完成 ${done} 个${failed.length > 0 ? `，失败 ${failed.length} 个（${failed.join('；')}）` : ''}`,
    );
    await load();
  }

  async function submitRename() {
    if (!renaming || renaming.name.trim() === '') return;
    try {
      await api('POST', '/logo/rename', { file: renaming.file, name: renaming.name.trim() });
      setRenaming(null);
      setMessage('已重命名');
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function remove(file: string) {
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
      <PageHead icon="logo" title="台标库" sub="上传、转码与匹配频道台标">
        {authed && (
          <>
            <button
              className="ghost"
              title="给所有静态 WebP 台标生成动效"
              disabled={busy}
              onClick={() => setPendingAction({ kind: 'animate-all' })}
            >
              一键全部动态化
            </button>
            <button
              className="ghost"
              title="用 .orig 备份还原所有动效台标"
              disabled={busy}
              onClick={() => setPendingAction({ kind: 'restore-all' })}
            >
              全部还原静态
            </button>
            <label
              className="ghost"
              title="选择图片上传；同名文件会被覆盖"
              style={{ cursor: 'pointer', height: 32, paddingTop: 7 }}
            >
              上传台标
              <input
                type="file"
                accept="image/*"
                multiple
                style={{ display: 'none' }}
                onChange={(event) => void upload(event.target.files)}
              />
            </label>
          </>
        )}
      </PageHead>
      <div className="card">
        <p className="muted">
          共 {logos.length} 个台标。公开地址形如 <code className="mono">/logo/&lt;文件名&gt;</code>。
        </p>
        {authed && (
          <div
            className="dropzone"
            onDragOver={(event) => event.preventDefault()}
            onDrop={(event) => {
              event.preventDefault();
              void upload(event.dataTransfer.files);
            }}
          >
            把图片拖到这里，或点页头「上传台标」。支持多选与 png / jpg / webp / bmp / avif。
          </div>
        )}
        {authed && (
          <div className="row">
            <label>
              最大边
              <Tip text="上传前按最长边等比缩放。超过这个值的图片会缩小，避免占用过多内存。" />
              <NumberBox
                min={16}
                max={2048}
                value={opts.maxEdge}
                onChange={(next) => setOpts({ ...opts, maxEdge: next })}
              />
            </label>
            <label>
              质量
              <Tip text="WebP 编码质量。数值越高越清晰，文件也越大。" />
              <NumberBox
                min={1}
                max={100}
                value={opts.quality}
                onChange={(next) => setOpts({ ...opts, quality: next })}
              />
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={opts.webp}
                onChange={(event) => setOpts({ ...opts, webp: event.target.checked })}
              />
              转 WebP
              <Tip text="在浏览器里先转成 WebP 再上传。透明背景会保留。" />
            </label>
            <span className="muted">
              上载前在浏览器里按最长边等比缩放并转码（服务端上载时不转码）。
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
        {logos.length === 0 && <Empty text="台标库为空。" icon="logo" />}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))',
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
                  <button
                    className="ghost"
                    title="修改显示名称，文件扩展名不变"
                    onClick={() => setRenaming({ file: logo.file, name: logo.name })}
                  >
                    重命名
                  </button>
                  <button
                    className="ghost"
                    title="把静态台标生成为动画 WebP"
                    onClick={() => {
                      setAnimReport(null);
                      setAnimating({ file: logo.file, effect: EFFECTS[0].value });
                    }}
                  >
                    动效
                  </button>
                  {logo.has_original && (
                    <button
                      className="ghost"
                      title="用 .orig 备份还原静态原图"
                      onClick={() => void restore(logo.file)}
                    >
                      还原
                    </button>
                  )}
                  <button
                    className="icon-btn"
                    title="删除台标；引用它的频道会变成坏引用"
                    onClick={() => setPendingAction({ kind: 'remove', file: logo.file })}
                  >
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
            <div className="field" style={{ width: 260 }}>
              <label>
                效果
                <Tip text="选择台标进入与退出的动画方式。生成后覆盖原图，原图备份到 .orig。" />
              </label>
              <Select
                ariaLabel="效果"
                title="选择台标进入与退出的动画方式"
                value={animating.effect}
                options={EFFECTS.map((item) => ({
                  value: item.value,
                  label: `${item.label}（${item.value}）`,
                }))}
                onChange={(next) => setAnimating({ ...animating, effect: next })}
              />
            </div>
            <button
              title="生成动画并覆盖当前台标，原图备份到 .orig"
              onClick={() => void generate(animating.file)}
            >
              生成
            </button>
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
          <h2>
            缺台标的频道
            <Tip text="选好文件后点「应用」，频道配置会写入对应的台标地址。" />
          </h2>
          <div className="table-wrap">
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
                        <Select
                          ariaLabel={`为 ${channel.name} 选择台标`}
                          title="选择要应用到该频道的台标文件"
                          value={picked[key] ?? suggestions[key] ?? ''}
                          options={[
                            { value: '', label: '（选择文件）' },
                            ...logos.map((logo) => ({ value: logo.file, label: logo.file })),
                          ]}
                          onChange={(next) => setPicked({ ...picked, [key]: next })}
                        />
                      </td>
                      <td className="right">
                        <button
                          className="ghost"
                          title="把所选台标写入频道配置"
                          onClick={() => void apply(channel)}
                        >
                          应用
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {pendingAction ? (
        <Confirm
          title={
            pendingAction.kind === 'animate-all'
              ? '批量生成动效？'
              : pendingAction.kind === 'restore-all'
                ? '批量还原静态？'
                : '删除台标？'
          }
          text={
            pendingAction.kind === 'animate-all'
              ? '所有静态 WebP 台标会生成动画。原图先备份到 .orig，完成后可还原。'
              : pendingAction.kind === 'restore-all'
                ? '所有带备份的台标会恢复为静态原图。动效会被覆盖。'
                : `删除「${pendingAction.file}」后，引用它的频道会变成坏引用。`
          }
          confirmText={
            pendingAction.kind === 'animate-all'
              ? '生成动效'
              : pendingAction.kind === 'restore-all'
                ? '还原静态'
                : '删除台标'
          }
          busy={busy}
          onCancel={() => setPendingAction(null)}
          onConfirm={() => {
            const action = pendingAction;
            setPendingAction(null);
            if (action.kind === 'animate-all') void animateAll();
            else if (action.kind === 'restore-all') void restoreAll();
            else void remove(action.file);
          }}
        />
      ) : null}

      {renaming ? (
        <Modal
          title="重命名台标"
          onClose={() => setRenaming(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setRenaming(null)}>
                取消
              </button>
              <button
                disabled={renaming.name.trim() === ''}
                onClick={() => void submitRename()}
              >
                保存
              </button>
            </>
          }
        >
          <label>
            新名称
            <Tip text="只改显示名称，文件扩展名保持不变。" />
          </label>
          <input
            autoFocus
            value={renaming.name}
            onChange={(event) => setRenaming({ ...renaming, name: event.target.value })}
          />
        </Modal>
      ) : null}
    </>
  );
}
