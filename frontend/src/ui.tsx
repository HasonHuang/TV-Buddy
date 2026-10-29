/** 通用界面组件：页头、弹窗、确认框、提示条、徽章、字段。 */

import { useCallback, useEffect, useState } from 'react';

import { Icon, type IconName } from './icons';

/** 分区页头：图标 + 标题 + 副标题，右侧放操作按钮。 */
export function PageHead({
  icon,
  title,
  sub,
  children,
}: {
  icon: IconName;
  title: string;
  sub?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="page-head">
      <div className="lead">
        <div className="title">
          <Icon name={icon} size={17} />
          {title}
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

/** 确认框。 */
export function Confirm({
  title,
  text,
  confirmText = '确认删除',
  onCancel,
  onConfirm,
}: {
  title: string;
  text: string;
  confirmText?: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal
      title={title}
      onClose={onCancel}
      footer={
        <>
          <button className="ghost" onClick={onCancel}>
            取消
          </button>
          <button className="danger" onClick={onConfirm}>
            {confirmText}
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

/** 表单字段：标签 + 可选说明 + 控件。 */
export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

/** 开关。 */
export function Switch({
  checked,
  onChange,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
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
