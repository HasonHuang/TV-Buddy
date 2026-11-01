/** 通用界面组件：页头、弹窗、确认框、提示条、徽章、字段。 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import { Icon, type IconName } from './icons';

/** 提示气泡。`?` 由伪元素绘制，不污染父元素文本内容。
 *
 * 气泡用 fixed 定位挂到 body：不会被卡片的 overflow 裁切，也不会撑出横向滚动条。
 */
export function Tip({ text, side = 'top' }: { text: string; side?: 'top' | 'bottom' }) {
  const ref = useRef<HTMLSpanElement>(null);
  const [bubble, setBubble] = useState<{
    left: number;
    top: number;
    placement: 'top' | 'bottom';
    width: number;
  } | null>(null);

  const show = () => {
    const node = ref.current;
    if (!node) return;
    const rect = node.getBoundingClientRect();
    const width = Math.min(268, window.innerWidth - 24);
    const left = Math.min(
      Math.max(rect.left + rect.width / 2, width / 2 + 12),
      window.innerWidth - width / 2 - 12,
    );
    const above = side === 'top' && rect.top > 96;
    setBubble({
      left,
      top: above ? rect.top - 8 : rect.bottom + 8,
      placement: above ? 'top' : 'bottom',
      width,
    });
  };

  const hide = () => setBubble(null);

  return (
    <>
      <span
        ref={ref}
        className="tip"
        tabIndex={0}
        role="img"
        aria-label={text}
        onMouseEnter={show}
        onMouseLeave={hide}
        onFocus={show}
        onBlur={hide}
        onClick={(event) => event.preventDefault()}
      />
      {bubble
        ? createPortal(
            <span
              className={`tip-bubble ${bubble.placement}`}
              style={{ left: bubble.left, top: bubble.top, maxWidth: bubble.width }}
              role="tooltip"
            >
              {text}
            </span>,
            document.body,
          )
        : null}
    </>
  );
}

/** 分区页头：图标 + 标题 + 副标题，右侧放操作按钮。 */
export function PageHead({
  icon,
  title,
  sub,
  help,
  sticky,
  children,
}: {
  icon: IconName;
  title: string;
  sub?: string;
  help?: string;
  sticky?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div className={sticky ? 'page-head sticky' : 'page-head'}>
      <div className="lead">
        <div className="title">
          <Icon name={icon} size={17} />
          {title}
          {help ? <Tip text={help} side="bottom" /> : null}
        </div>
        {sub && <div className="sub">{sub}</div>}
      </div>
      {children && <div className="actions">{children}</div>}
    </div>
  );
}

/** 弹窗壳。遮罩点击与 Esc 都能关闭。 */
export function Modal({
  title,
  onClose,
  children,
  footer,
}: {
  title: string;
  onClose: () => void;
  children: React.ReactNode;
  footer?: React.ReactNode;
}) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={(event) => event.stopPropagation()}>
        <div className="modal-head">
          <h3>{title}</h3>
          <button className="icon-btn" onClick={onClose} aria-label="关闭">
            <Icon name="close" size={15} />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        <div className="modal-foot">
          {footer ?? (
            <button className="ghost" onClick={onClose}>
              关闭
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/** 确认框。危险操作默认红色按钮。 */
export function Confirm({
  title,
  text,
  confirmText = '确认删除',
  tone = 'danger',
  busy,
  onCancel,
  onConfirm,
}: {
  title: string;
  text: string;
  confirmText?: string;
  tone?: 'danger' | 'primary';
  busy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal
      title={title}
      onClose={onCancel}
      footer={
        <>
          <button className="ghost" onClick={onCancel} disabled={busy}>
            取消
          </button>
          <button
            className={tone === 'danger' ? 'danger' : ''}
            onClick={onConfirm}
            disabled={busy}
          >
            {busy ? '处理中…' : confirmText}
          </button>
        </>
      }
    >
      <p className="muted">{text}</p>
    </Modal>
  );
}

/** 徽章。 */
export function Badge({ tone, children }: { tone: string; children: React.ReactNode }) {
  return <span className={`badge ${tone}`}>{children}</span>;
}

/** 卡片：标题 + 说明 + 右侧操作 + 内容。 */
export function Card({
  title,
  note,
  tip,
  actions,
  wide,
  className,
  children,
}: {
  title?: string;
  note?: string;
  tip?: string;
  actions?: React.ReactNode;
  wide?: boolean;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div className={`card${wide ? ' wide' : ''}${className ? ` ${className}` : ''}`}>
      {(title || actions) && (
        <div className="card-head">
          <div className="card-head-text">
            {title && (
              <h2>
                {title}
                {tip ? <Tip text={tip} /> : null}
              </h2>
            )}
            {note ? <span className="muted">{note}</span> : null}
          </div>
          {actions ? <div className="card-actions">{actions}</div> : null}
        </div>
      )}
      {children}
    </div>
  );
}

/** 表单字段：标签 + 可选提示/说明 + 控件。 */
export function Field({
  label,
  hint,
  tip,
  children,
}: {
  label: string;
  hint?: string;
  tip?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      {children}
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

/** 空状态。 */
export function Empty({ text, icon = 'file' }: { text: string; icon?: IconName }) {
  return (
    <div className="empty">
      <Icon name={icon} size={20} />
      <span>{text}</span>
    </div>
  );
}

/** 开关。 */
export function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-label={label}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span />
    </label>
  );
}

/** 提示条。`kind` 为 `ok` 或 `err`。返回的 `ok`/`err` 回调身份稳定。 */
export function useToast() {
  const [toast, setToast] = useState<{ kind: 'ok' | 'err'; text: string } | null>(null);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 2600);
    return () => window.clearTimeout(timer);
  }, [toast]);

  const ok = useCallback((text: string) => setToast({ kind: 'ok', text }), []);
  const err = useCallback((text: string) => setToast({ kind: 'err', text }), []);

  const node = toast ? <div className={`toast ${toast.kind}`}>{toast.text}</div> : null;

  return { node, ok, err };
}
