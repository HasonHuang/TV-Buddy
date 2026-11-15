/** Python 脚本管理：解析脚本与 EPG 采集脚本共用列表、上传、删除与开发指南。
 *
 * 原来上传入口只在运维分区的一张小卡片里。用户改频道地址来源或节目源类型时
 * 找不到入口。本组件把同一套管理能力放进选择脚本的地方。
 */

import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';

import { api } from './api';
import { Icon } from './icons';
import { Badge, Confirm, Field, Modal, Tip } from './ui';

export type ScriptKind = 'resolver' | 'epg';

type ScriptEntry = { name: string; bytes: number };

type KindMeta = {
  /** 卡片与弹窗标题。 */
  title: string;
  /** 卡片副标题。 */
  note: string;
  list: string;
  save: string;
  remove: string;
  /** 脚本目录，显示给用户。 */
  directory: string;
  guide: ReactNode;
};

const RESOLVER_GUIDE = (
  <>
    <ul className="script-guide">
      <li>
        位置：服务端缓存目录的 <code>resolvers/</code>。只接受 <code>.py</code> 文件。
      </li>
      <li>
        执行：<code>python3 &lt;脚本&gt; &lt;脚本参数&gt;</code>。工作目录是脚本所在目录。
      </li>
      <li>
        参数：<code>sys.argv[1]</code> 是频道表单里的「脚本参数」。参数可以为空。
      </li>
      <li>
        输出：标准输出的最后一行是上游地址。地址必须以 <code>http://</code> 或{' '}
        <code>https://</code> 开头。
      </li>
      <li>失败：脚本非零退出、超时（30 秒）或输出不是地址时，沿用上一次成功的结果。</li>
      <li>缓存：结果按频道表单的「缓存秒数」复用。0 表示每次播放都重新运行。</li>
      <li>
        环境：容器默认不装 Python。请先安装 <code>python3</code>，否则脚本不能运行。
      </li>
    </ul>
    <p className="muted">最小示例：</p>
    <pre className="script-sample">{`#!/usr/bin/env python3
import sys

arg = sys.argv[1] if len(sys.argv) > 1 else ''
print('https://example.com/live.m3u8')`}</pre>
  </>
);

const EPG_GUIDE = (
  <>
    <ul className="script-guide">
      <li>
        位置：服务端缓存目录的 <code>epg/scripts/</code>。只接受 <code>.py</code> 文件。
      </li>
      <li>
        执行：<code>python3 &lt;脚本&gt;</code>。工作目录是脚本所在目录。可在「系统设置」里改
        Python 命令。
      </li>
      <li>
        输出：标准输出写 XMLTV 正文。标准输出为空时，按节目源的「产物文件」读取文件。
      </li>
      <li>超时：节目源的「超时（秒）」。0 表示使用全局脚本超时。</li>
      <li>
        开关：「系统设置」的「允许脚本源」必须开启。关闭后采集报错「已禁止脚本源」。
      </li>
      <li>失败：保留上一次采集的节目单。运行日志记录错误。</li>
      <li>
        环境：容器默认不装 Python。请先安装 <code>python3</code>，否则脚本不能运行。
      </li>
    </ul>
    <p className="muted">最小示例：</p>
    <pre className="script-sample">{`#!/usr/bin/env python3
print('<?xml version="1.0" encoding="UTF-8"?>')
print('<tv generator-info-name="custom"></tv>')`}</pre>
  </>
);

const KINDS: Record<ScriptKind, KindMeta> = {
  resolver: {
    title: '解析脚本',
    note: '频道地址来源选「解析脚本」时使用。',
    list: '/resolver/scripts',
    save: '/resolver/script',
    remove: '/resolver/remove',
    directory: 'resolvers/',
    guide: RESOLVER_GUIDE,
  },
  epg: {
    title: '采集脚本',
    note: '节目指南的脚本源使用。',
    list: '/epg/scripts',
    save: '/epg/script',
    remove: '/epg/script-delete',
    directory: 'epg/scripts/',
    guide: EPG_GUIDE,
  },
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * 脚本管理面板。
 *
 * `onChanged` 在保存或删除后调用。保存时带上文件名，方便调用方直接选中新脚本。
 */
export function ScriptManager({
  kind,
  initialName = '',
  guideOpen = false,
  onChanged,
}: {
  kind: ScriptKind;
  initialName?: string;
  guideOpen?: boolean;
  onChanged?: (saved?: string) => void;
}) {
  const meta = KINDS[kind];
  const [scripts, setScripts] = useState<ScriptEntry[]>([]);
  /** EPG 接口带 Python 版本。`undefined` = 还没读到。 */
  const [python, setPython] = useState<string | null | undefined>(undefined);
  const [name, setName] = useState(initialName);
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [pendingSave, setPendingSave] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const fileRef = useRef<HTMLInputElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  const load = useCallback(async () => {
    try {
      if (kind === 'epg') {
        const body = await api<{
          python?: string | null;
          scripts?: { name: string; size: number }[];
        }>('GET', meta.list);
        setPython(body.python ?? null);
        setScripts(
          (body.scripts ?? []).map((item) => ({ name: item.name, bytes: item.size ?? 0 })),
        );
      } else {
        const body = await api<{ scripts?: { name: string; bytes: number }[] }>(
          'GET',
          meta.list,
        );
        setScripts(body.scripts ?? []);
      }
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, [kind, meta.list]);

  useEffect(() => {
    void load();
  }, [load]);

  /** 同名文件已存在时先确认。管理页不读取旧正文，覆盖后不能恢复。 */
  const requestSave = () => {
    const trimmed = name.trim();
    if (trimmed === '') {
      setError('请填写脚本文件名');
      return;
    }
    if (content.trim() === '') {
      setError('脚本正文不能为空');
      return;
    }
    const fileName = trimmed.endsWith('.py') ? trimmed : `${trimmed}.py`;
    setError('');
    if (scripts.some((script) => script.name === fileName)) setPendingSave(fileName);
    else void commitSave(fileName);
  };

  const commitSave = async (fileName: string) => {
    setBusy(true);
    setMessage('');
    setError('');
    try {
      await api('POST', meta.save, { name: fileName, content });
      setName(fileName);
      setMessage(`脚本 ${fileName} 已保存。`);
      if (fileRef.current) fileRef.current.value = '';
      await load();
      onChanged?.(fileName);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (target: string) => {
    setMessage('');
    setError('');
    try {
      await api('POST', meta.remove, { name: target });
      if (name === target) {
        setName('');
        setContent('');
      }
      setMessage(`脚本 ${target} 已删除。`);
      await load();
      onChanged?.();
    } catch (err) {
      setError(errorText(err));
    }
  };

  /** 把本机文件读进正文。文件较大时浏览器仍能处理。 */
  const readFile = async (file: File) => {
    if (!file.name.endsWith('.py')) {
      setError('只接受 .py 文件');
      return;
    }
    try {
      setContent(await file.text());
      setName(file.name);
      setMessage(`已读取 ${file.name}。请检查正文后保存。`);
      setError('');
      textRef.current?.focus();
    } catch (err) {
      setError(errorText(err));
    } finally {
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  /** 选中已有文件：填入文件名，正文留空。保存前必须粘贴完整内容。 */
  const pick = (target: string) => {
    setName(target);
    setContent('');
    setMessage('');
    setError('');
    textRef.current?.focus();
  };

  return (
    <div className="script-manager">
      <div className="row between">
        <span className="muted">
          {scripts.length} 个脚本 · {meta.directory}
        </span>
        <div className="row">
          {kind === 'epg' && python !== undefined ? (
            python ? (
              <Badge tone="ok">{python}</Badge>
            ) : (
              <Badge tone="warn">未检测到 python3</Badge>
            )
          ) : null}
          <button
            className="ghost"
            title="重新读取脚本列表"
            onClick={() => void load()}
          >
            <Icon name="refresh" size={13} />
            刷新
          </button>
        </div>
      </div>

      {scripts.length === 0 ? (
        <p className="muted">还没有脚本。填写文件名与正文，然后保存。</p>
      ) : (
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
              {scripts.map((script) => (
                <tr key={script.name}>
                  <td className="mono">{script.name}</td>
                  <td className="right">
                    {Math.max(1, Math.round(script.bytes / 1024))} KB
                  </td>
                  <td className="right">
                    <div className="row" style={{ justifyContent: 'flex-end' }}>
                      <button
                        className="ghost"
                        title="用当前正文覆盖这个脚本"
                        onClick={() => pick(script.name)}
                      >
                        覆盖
                      </button>
                      <button
                        className="ghost danger-text"
                        title="删除这个脚本"
                        onClick={() => setPendingDelete(script.name)}
                      >
                        删除
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="row between script-editor-head">
        <span className="section-title">新建或覆盖</span>
        <Tip text="同名文件会被整个覆盖。管理页不显示旧正文，请先备份需要保留的内容。" />
      </div>
      <div className="fields">
        <Field
          label="文件名"
          tip="脚本保存在服务端。只接受 .py 文件。同名文件会被整个覆盖。"
          hint="扩展名 .py 自动补全。"
        >
          <input
            aria-label="脚本文件名"
            placeholder="mysource.py"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
      </div>
      <div className="row">
        <label className="ghost file-pick" title="读取本机 .py 文件到正文">
          <Icon name="upload" size={14} />
          从文件读取
          <input
            ref={fileRef}
            type="file"
            accept=".py,text/x-python"
            aria-label="选择 Python 脚本文件"
            onChange={(event) => {
              const selected = event.target.files?.[0];
              if (selected) void readFile(selected);
            }}
          />
        </label>
        <span className="muted">读取后请检查正文，再保存。</span>
      </div>
      <textarea
        ref={textRef}
        aria-label="脚本正文"
        title="Python 脚本正文。保存后按使用位置运行"
        placeholder="#!/usr/bin/env python3&#10;"
        rows={8}
        value={content}
        onChange={(event) => setContent(event.target.value)}
        style={{ width: '100%', marginTop: 8 }}
      />
      <div className="row between" style={{ marginTop: 8 }}>
        <span className="muted">服务端直接运行保存后的文件。只上传可信代码。</span>
        <button
          onClick={requestSave}
          disabled={busy || name.trim() === '' || content.trim() === ''}
          title="保存脚本文件；同名文件会被覆盖"
        >
          {busy ? '保存中…' : '保存脚本'}
        </button>
      </div>

      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}

      <details className="block" open={guideOpen}>
        <summary>
          <Icon name="file" size={13} />
          脚本开发指南
        </summary>
        {meta.guide}
      </details>

      {pendingSave ? (
        <Confirm
          title="覆盖脚本？"
          text={`「${pendingSave}」已存在。保存会用当前正文替换整个文件。旧内容不能恢复。`}
          confirmText="覆盖保存"
          busy={busy}
          onCancel={() => setPendingSave(null)}
          onConfirm={() => {
            const target = pendingSave;
            setPendingSave(null);
            void commitSave(target);
          }}
        />
      ) : null}
      {pendingDelete ? (
        <Confirm
          title="删除脚本？"
          text={`脚本「${pendingDelete}」删除后，引用它的位置会运行失败。这个操作不能撤销。`}
          confirmText="删除脚本"
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => {
            const target = pendingDelete;
            setPendingDelete(null);
            void remove(target);
          }}
        />
      ) : null}
    </div>
  );
}

/** 弹窗形式。选择脚本的表单用这个入口，不离开当前页面。 */
export function ScriptManagerModal({
  kind,
  initialName,
  onClose,
  onChanged,
}: {
  kind: ScriptKind;
  initialName?: string;
  onClose: () => void;
  onChanged?: (saved?: string) => void;
}) {
  return (
    <Modal
      title={KINDS[kind].title}
      onClose={onClose}
      footer={
        <button className="ghost" onClick={onClose}>
          完成
        </button>
      }
    >
      <p className="muted">{KINDS[kind].note}</p>
      <ScriptManager kind={kind} initialName={initialName} guideOpen onChanged={onChanged} />
    </Modal>
  );
}

/** 卡片形式。系统状态页与节目指南页用这个入口。 */
export function ScriptManagerCard({ kind }: { kind: ScriptKind }) {
  const meta = KINDS[kind];
  return (
    <div className="card">
      <div className="card-head">
        <div className="card-head-text">
          <h2>{meta.title}</h2>
          <span className="muted">{meta.note}</span>
        </div>
      </div>
      <ScriptManager kind={kind} />
    </div>
  );
}
