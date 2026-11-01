/** 通用界面组件：页头、弹窗、确认框、提示条、徽章、字段。 */

import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import { Icon, type IconName } from './icons';
import { currentTheme, toggleTheme, watchSystemTheme, type Theme } from './theme';

/** 明暗主题开关。
 *
 * 显示当前主题的反向图标：深色时给太阳，浅色时给月亮。
 * `showLabel` 在侧栏显示目标主题的文字，便于发现。
 * 点一次即固定用户的选择，之后不再跟随系统。
 */
export function ThemeToggle({
  className = '',
  showLabel = false,
}: {
  className?: string;
  showLabel?: boolean;
}) {
  const [theme, setTheme] = useState<Theme>(() => currentTheme());

  useEffect(
    () => watchSystemTheme((next) => setTheme(next)),
    [],
  );

  const label = theme === 'dark' ? '切换到浅色主题' : '切换到深色主题';
  return (
    <button
      type="button"
      className={`ghost icon-btn theme-toggle${className ? ` ${className}` : ''}`}
      title={label}
      aria-label={label}
      onClick={() => setTheme(toggleTheme())}
    >
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={15} />
      {showLabel ? <span className="nav-label">{theme === 'dark' ? '浅色' : '深色'}</span> : null}
    </button>
  );
}


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

/** 自绘下拉框。
 *
 * 原生 `<select>` 的样式受系统限制，各平台外观不一。这里用按钮 + 列表实现，
 * 换来统一的圆角、字重与明暗配色。键盘可用：方向键、Home、End、Enter、Esc。
 *
 * 选项列表不是输入框，值由调用方持有；本组件只负责显示与回调。
 */
export function Select({
  value,
  options,
  onChange,
  disabled,
  ariaLabel,
  placeholder = '请选择',
  className = '',
  align = 'left',
  compact,
  width,
  title,
}: {
  value: string;
  options: { value: string; label: string }[];
  onChange: (next: string) => void;
  disabled?: boolean;
  ariaLabel?: string;
  placeholder?: string;
  className?: string;
  /** 菜单相对按钮的对齐方向。右侧字段用 `right`，避免贴边溢出。 */
  align?: 'left' | 'right';
  /** 选项文字短时用固定窄宽，与紧凑数字框并列时不撑满整行。 */
  compact?: boolean;
  /** 固定宽度，数字为 px。设置后不再随父容器伸展。 */
  width?: number | string;
  /** 悬停提示。内容长时用。 */
  title?: string;
}) {
  const id = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const optionRefs = useRef<(HTMLLIElement | null)[]>([]);
  const typed = useRef({ text: '', at: 0 });
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [dropUp, setDropUp] = useState(false);

  const selected = options.findIndex((option) => option.value === value);

  const close = (focusButton = false) => {
    setOpen(false);
    if (focusButton) btnRef.current?.focus();
  };

  const openMenu = () => {
    if (disabled) return;
    const rect = btnRef.current?.getBoundingClientRect();
    if (rect) setDropUp(window.innerHeight - rect.bottom < 268 && rect.top > 268);
    setActive(selected >= 0 ? selected : 0);
    setOpen(true);
  };

  // 打开后把焦点移到当前选项。用 `open` 作依赖即可：列表刚挂载，ref 已就位。
  useEffect(() => {
    if (!open) return;
    optionRefs.current[active]?.focus();
  }, [open]);

  // 点击外部关闭。用 pointerdown，早于焦点转移，避免菜单先失焦再点不中。
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    return () => document.removeEventListener('pointerdown', onPointerDown);
  }, [open]);

  // 页面滚动或窗口改尺寸时关闭，菜单不跟着按钮偏移。
  // 菜单自身滚动不算：长列表要能滚到底。
  useEffect(() => {
    if (!open) return;
    const onMove = (event?: Event) => {
      if (event && rootRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    const onResize = () => setOpen(false);
    window.addEventListener('resize', onResize);
    window.addEventListener('scroll', onMove, true);
    return () => {
      window.removeEventListener('resize', onResize);
      window.removeEventListener('scroll', onMove, true);
    };
  }, [open]);

  const move = (next: number) => {
    const count = options.length;
    if (!count) return;
    const index = ((next % count) + count) % count;
    setActive(index);
    optionRefs.current[index]?.focus();
  };

  const choose = (index: number) => {
    const option = options[index];
    if (!option) return;
    onChange(option.value);
    close(true);
  };

  const onMenuKeyDown = (event: React.KeyboardEvent<HTMLUListElement>) => {
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        move(active + 1);
        return;
      case 'ArrowUp':
        event.preventDefault();
        move(active - 1);
        return;
      case 'Home':
        event.preventDefault();
        move(0);
        return;
      case 'End':
        event.preventDefault();
        move(options.length - 1);
        return;
      case 'Enter':
      case ' ':
        event.preventDefault();
        choose(active);
        return;
      case 'Escape':
        event.preventDefault();
        event.stopPropagation();
        close(true);
        return;
      case 'Tab':
        close();
        return;
      default:
        break;
    }

    // 键入首字跳转。中文与英文都用 `startsWith`，700 ms 内连续输入算一次查询。
    if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      const now = Date.now();
      typed.current.text =
        now - typed.current.at > 700 ? event.key : typed.current.text + event.key;
      typed.current.at = now;
      const query = typed.current.text.toLowerCase();
      const start = active >= 0 ? active : 0;
      for (let step = 0; step < options.length; step += 1) {
        const index = (start + step) % options.length;
        if (options[index].label.toLowerCase().startsWith(query)) {
          event.preventDefault();
          move(index);
          return;
        }
      }
    }
  };

  return (
    <div
      ref={rootRef}
      className={`select${compact || width !== undefined ? ' compact' : ''}${open ? ' open' : ''}${className ? ` ${className}` : ''}`}
      style={width !== undefined ? { width } : undefined}
    >
      <button
        ref={btnRef}
        type="button"
        className="select-btn"
        role="combobox"
        aria-controls={`${id}-menu`}
        aria-expanded={open}
        aria-haspopup="listbox"
        aria-label={ariaLabel}
        title={title}
        disabled={disabled}
        onClick={() => (open ? close(true) : openMenu())}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            openMenu();
          }
        }}
      >
        <span className={`select-value${selected < 0 ? ' placeholder' : ''}`}>
          {selected >= 0 ? options[selected].label : placeholder}
        </span>
        <Icon name="down" size={14} className="chev" />
      </button>

      {open && (
        <ul
          id={`${id}-menu`}
          className={`select-menu${dropUp ? ' drop-up' : ''}${align === 'right' ? ' right' : ''}`}
          role="listbox"
          aria-label={ariaLabel}
          onKeyDown={onMenuKeyDown}
        >
          {options.map((option, index) => (
            <li
              key={option.value}
              ref={(node) => {
                optionRefs.current[index] = node;
              }}
              role="option"
              aria-selected={option.value === value}
              tabIndex={-1}
              className={`select-option${option.value === value ? ' selected' : ''}`}
              onFocus={() => setActive(index)}
              onClick={() => choose(index)}
            >
              <span>{option.label}</span>
              {option.value === value ? <Icon name="check" size={14} /> : null}
            </li>
          ))}
        </ul>
      )}
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
