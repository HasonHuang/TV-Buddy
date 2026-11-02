/** 系统设置：字段级表单。
 *
 * 一次 `GET /panel/api/config` 取整份配置到草稿，改完用顶部「保存设置」一次
 * `PUT` 回去（对齐目标 p12：修改后点击保存生效）。未覆盖的段保留最下面的
 * 「其余字段（JSON）」兜底编辑。
 *
 * 布局基线：`docs/design/reference/img/p12-settings.png`。
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

import { api, putConfig } from './api';
import { Icon } from './icons';
import { RecommendCard, type Recommend } from './lowcfg';
import { BackupPage } from './ops';
import { SystemPage } from './system';
import { Card, Confirm, PageHead, Select, Tip, useToast } from './ui';

type Cfg = Record<string, unknown>;

/** 按 `a.b.c` 取子值。 */
function getPath(root: Cfg, path: string): unknown {
  return path.split('.').reduce<unknown>((acc, key) => {
    if (acc && typeof acc === 'object') return (acc as Cfg)[key];
    return undefined;
  }, root);
}

/** 按 `a.b.c` 不可变写子值。 */
function setPath(root: Cfg, path: string, value: unknown): Cfg {
  const [head, ...rest] = path.split('.');
  if (rest.length === 0) return { ...root, [head]: value };
  const child = (root[head] ?? {}) as Cfg;
  return { ...root, [head]: setPath(child, rest.join('.'), value) };
}

function asNumber(root: Cfg, path: string): number {
  const value = getPath(root, path);
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

function asText(root: Cfg, path: string): string {
  const value = getPath(root, path);
  if (value === null || value === undefined) return '';
  return typeof value === 'string' ? value : String(value);
}

function asBool(root: Cfg, path: string): boolean {
  return getPath(root, path) === true;
}

function asList(root: Cfg, path: string): string[] {
  const value = getPath(root, path);
  return Array.isArray(value) ? value.map((item) => String(item)) : [];
}

type DraftValue = { draft: Cfg; update: (path: string, value: unknown) => void };

const DraftCtx = createContext<DraftValue | null>(null);

function useDraft(): DraftValue {
  const value = useContext(DraftCtx);
  if (!value) throw new Error('DraftCtx 未提供');
  return value;
}

/** 数字输入。 */
function Num({
  label,
  path,
  hint,
  tip,
  unit,
  step,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  unit?: string;
  step?: number;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="field-control">
        <input
          className="num"
          type="number"
          min={0}
          step={step ?? 1}
          value={String(asNumber(draft, path))}
          onChange={(event) => {
            const next = Number(event.target.value);
            update(path, Number.isFinite(next) ? next : 0);
          }}
        />
        {unit ? <span className="field-unit">{unit}</span> : null}
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 文本输入。 */
function Text({
  label,
  path,
  hint,
  tip,
  placeholder,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  placeholder?: string;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="field-control">
        <input
          value={asText(draft, path)}
          placeholder={placeholder}
          onChange={(event) => update(path, event.target.value)}
        />
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 下拉选择。 */
function Pick({
  label,
  path,
  hint,
  tip,
  options,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  options: { value: string; label: string }[];
}) {
  const { draft, update } = useDraft();
  // 最短选项也短时收窄下拉，避免「开启 / 关闭」占满整行。
  const compact = options.every((option) => option.label.length <= 8);
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="field-control">
        <Select
          value={asText(draft, path)}
          options={options}
          onChange={(next) => update(path, next)}
          ariaLabel={label}
          compact={compact}
        />
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 布尔下拉：值仍是布尔，只是外观与目标一致。 */
function BoolPick({
  label,
  path,
  hint,
  tip,
  onLabel = '开启',
  offLabel = '关闭',
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  onLabel?: string;
  offLabel?: string;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="field-control">
        <Select
          value={asBool(draft, path) ? 'on' : 'off'}
          options={[
            { value: 'on', label: onLabel },
            { value: 'off', label: offLabel },
          ]}
          onChange={(next) => update(path, next === 'on')}
          ariaLabel={label}
          compact
        />
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 复选框。 */
function Toggle({
  label,
  path,
  hint,
  tip,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field field-check">
      <label className="check">
        <input
          type="checkbox"
          checked={asBool(draft, path)}
          onChange={(event) => update(path, event.target.checked)}
        />
        <span>
          {label}
          {tip ? <Tip text={tip} /> : null}
        </span>
      </label>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 多选复选框（伪装格式）。 */
function Multi({
  label,
  path,
  hint,
  tip,
  options,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  options: string[];
}) {
  const { draft, update } = useDraft();
  const chosen = asList(draft, path);
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="checkboxes">
        {options.map((option) => (
          <label key={option} className="check">
            <input
              type="checkbox"
              checked={chosen.includes(option)}
              onChange={(event) =>
                update(
                  path,
                  event.target.checked
                    ? [...chosen, option]
                    : chosen.filter((item) => item !== option),
                )
              }
            />
            {option}
          </label>
        ))}
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 多行文本 ⇄ 字符串数组（一行一条）。
 *
 * 编辑期间保留原文本（否则回车会被「去空行」吃掉），失焦时才写回草稿并丢掉空行。
 */
function Lines({
  label,
  path,
  hint,
  tip,
  rows,
  placeholder,
}: {
  label: string;
  path: string;
  hint?: string;
  tip?: string;
  rows?: number;
  placeholder?: string;
}) {
  const { draft, update } = useDraft();
  const stored = asList(draft, path).join('\n');
  const [text, setText] = useState(stored);
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) setText(stored);
  }, [stored, editing]);

  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <textarea
        rows={rows ?? 3}
        spellCheck={false}
        placeholder={placeholder}
        value={text}
        onFocus={() => setEditing(true)}
        onChange={(event) => setText(event.target.value)}
        onBlur={() => {
          setEditing(false);
          update(
            path,
            text
              .split('\n')
              .map((line) => line.trimEnd())
              .filter((line) => line !== ''),
          );
        }}
      />
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 标签多选（字幕目标语言）。选中集写回数组；`primaryPath` 同步首个选中项。 */
function Chips({
  label,
  path,
  primaryPath,
  hint,
  tip,
  options,
}: {
  label: string;
  path: string;
  primaryPath?: string;
  hint?: string;
  tip?: string;
  options: { value: string; label: string }[];
}) {
  const { draft, update } = useDraft();
  const chosen = asList(draft, path);
  const toggle = (value: string) => {
    const next = chosen.includes(value)
      ? chosen.filter((item) => item !== value)
      : [...chosen, value];
    update(path, next);
    if (primaryPath) update(primaryPath, next[0] ?? '');
  };
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="row" style={{ flexWrap: 'wrap' }}>
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            className={chosen.includes(option.value) ? '' : 'ghost'}
            onClick={() => toggle(option.value)}
          >
            {option.label}
          </button>
        ))}
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 滑杆（字幕位置）。 */
function Range({
  label,
  path,
  min,
  max,
  unit,
  hint,
  tip,
}: {
  label: string;
  path: string;
  min: number;
  max: number;
  unit?: string;
  hint?: string;
  tip?: string;
}) {
  const { draft, update } = useDraft();
  const value = asNumber(draft, path);
  return (
    <div className="field">
      <label>
        {label}
        {tip ? <Tip text={tip} /> : null}
      </label>
      <div className="field-control">
        <input
          type="range"
          min={min}
          max={max}
          value={value}
          onChange={(event) => update(path, Number(event.target.value))}
        />
        <span className="field-unit">
          {value}
          {unit ?? ''}
        </span>
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error'];

const CATCHUP_TEMPLATES = [
  { value: 'guonei', label: '国产（DIYP / 影音仓库 / TVBox / OK）' },
  { value: 'dollar', label: '国外（${start}-${end}）' },
  { value: 'unix', label: 'Kodi（{utc}-{utcend}）' },
  { value: 'both', label: '两者都带' },
];

/** 字幕字号预设；`[subtitle].style.font_size` 存字符串百分比。 */
const FONT_SIZES = ['60%', '70%', '80%', '90%', '100%', '110%', '120%', '150%'].map((size) => ({
  value: size,
  label: size === '80%' ? '80%（默认）' : size,
}));

/** 字幕颜色；空串 = 跟随播放器默认。取值为十六进制，目标的具体预设未观测。 */
const SUBTITLE_COLORS = [
  { value: '', label: '默认颜色' },
  { value: '#ffffff', label: '白色' },
  { value: '#000000', label: '黑色' },
  { value: '#ffff00', label: '黄色' },
  { value: '#00ffff', label: '青色' },
  { value: '#ff0000', label: '红色' },
];

/** 字幕机翻的目标语言（按 p12 参考截图的顺序）。 */
const SUBTITLE_LANGS = [
  { value: 'zh-cn', label: '简体中文' },
  { value: 'zh-tw', label: '繁體中文' },
  { value: 'ja', label: '日本語' },
  { value: 'ko', label: '한국어' },
  { value: 'en', label: 'English' },
  { value: 'es', label: 'Español' },
  { value: 'fr', label: 'Français' },
  { value: 'de', label: 'Deutsch' },
  { value: 'ru', label: 'Русский' },
  { value: 'he', label: 'עברית' },
  { value: 'vi', label: 'Tiếng Việt' },
];

/** 递归统计两棵配置树的差异项数；改回原值后归零。 */
function countDiff(a: unknown, b: unknown): number {
  if (a === b) return 0;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return 1;
    let count = 0;
    for (let index = 0; index < a.length; index += 1) count += countDiff(a[index], b[index]);
    return count;
  }
  if (a && b && typeof a === 'object' && typeof b === 'object') {
    const keys = new Set([...Object.keys(a as Cfg), ...Object.keys(b as Cfg)]);
    let count = 0;
    for (const key of keys) {
      count += countDiff((a as Cfg)[key], (b as Cfg)[key]);
    }
    return count;
  }
  return 1;
}

/** 系统设置页。`onDirtyChange` 供外壳在切分区前拦截未保存修改。 */
export function SettingsPage({
  onDirtyChange,
}: {
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const [draft, setDraft] = useState<Cfg | null>(null);
  const [baseline, setBaseline] = useState<Cfg | null>(null);
  const [recommend, setRecommend] = useState<Recommend | null>(null);
  const [busy, setBusy] = useState('');
  const [confirmReload, setConfirmReload] = useState(false);
  const [confirmFlush, setConfirmFlush] = useState(false);
  const [confirmSubClear, setConfirmSubClear] = useState(false);
  const { node, ok, err } = useToast();

  const changed = useMemo(
    () => (baseline && draft ? countDiff(baseline, draft) : 0),
    [baseline, draft],
  );
  const dirty = changed > 0;

  useEffect(() => {
    onDirtyChange?.(dirty);
  }, [dirty, onDirtyChange]);

  const load = useCallback(async () => {
    try {
      const next = await api<Cfg>('GET', '/config');
      setDraft(next);
      setBaseline(next);
      try {
        const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
        setRecommend(stats.recommend);
      } catch {
        setRecommend(null);
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const update = useCallback((path: string, value: unknown) => {
    setDraft((prev) => (prev ? setPath(prev, path, value) : prev));
  }, []);

  const value = useMemo<DraftValue | null>(
    () => (draft ? { draft, update } : null),
    [draft, update],
  );

  const save = useCallback(async () => {
    if (!draft) return;
    setBusy('save');
    try {
      const next = draft;
      await putConfig(next);
      setDraft(next);
      setBaseline(next);
      ok('设置已保存');
      try {
        const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
        setRecommend(stats.recommend);
      } catch {
        // 推荐值取不到不影响保存结果
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  }, [draft, err, ok]);

  // Ctrl/Cmd+S：用 ref 取最新的保存函数，监听只注册一次。
  const saveRef = useRef<() => void>(() => {});
  saveRef.current = () => {
    if (draft && dirty && busy !== 'save') void save();
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        saveRef.current();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [dirty]);

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

  const flush = async () => {
    setBusy('flush');
    try {
      await api('POST', '/cache/flush');
      ok('本地缓存已清空');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const clearTranslated = async () => {
    setBusy('subclear');
    try {
      await api('POST', '/subtitle/translate/clear');
      ok('译文缓存已清空');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const testMtran = async () => {
    setBusy('mtran');
    try {
      const result = await api<{ ok: boolean; base?: string; targets?: { error?: string }[] }>(
        'POST',
        '/subtitle/mtran/test',
        { text: 'hello' },
      );
      if (result.ok) {
        ok(`机翻连通正常${result.base ? `（${result.base}）` : ''}`);
      } else {
        err(result.targets?.[0]?.error || '机翻测试失败');
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const ttlHint =
    asNumber(draft ?? {}, 'cdn.segment_max_age_secs') === 0
      ? `自动（当前 ${recommend?.ttl_secs ?? '—'}s）`
      : '手动值，覆盖自动推荐';
  const capHint =
    asNumber(draft ?? {}, 'cache.gc.max_disk_mb') === 0
      ? `自动（当前 ${recommend?.disk_cap_mb ?? '—'} MB）`
      : '手动值，覆盖自动推荐';

  return (
    <div className="settings-page">
      <PageHead
        icon="settings"
        title="系统设置"
        sub="修改后点击保存生效；配置保存在本地数据库。"
        help="所有改动先写入本地草稿，点击「保存设置」后整份提交。Ctrl/Cmd+S 也可保存。"
        sticky
      >
        {dirty ? <span className="badge warn">未保存 {changed} 项</span> : null}
        <button
          className="ghost"
          title="从服务端重新读取当前配置；未保存的修改会丢失"
          onClick={() => (dirty ? setConfirmReload(true) : void load())}
        >
          <Icon name="refresh" size={14} />
          重新读取
        </button>
        <button
          disabled={busy === 'save' || !draft || !dirty}
          title="保存整份配置（Ctrl/Cmd+S）"
          onClick={() => void save()}
        >
          {busy === 'save' ? '保存中…' : dirty ? '保存设置 *' : '保存设置'}
        </button>
      </PageHead>

      {!draft || !value ? (
        <div className="card">
          <p className="muted">读取配置中…</p>
        </div>
      ) : (
        <DraftCtx.Provider value={value}>
          <div className="settings-grid">
            <Card
              title="服务器"
              note="监听、跳转与订阅头"
              tip="这些值影响服务启动与所有订阅输出。带「需重启」的字段保存后不会立即生效。"
            >
              <div className="fields">
                <Text
                  label="监听地址"
                  path="server.bind"
                  tip="服务监听的地址与端口，例如 [::]:9900。保存后需重启进程。"
                  hint="改后需重启"
                />
                <Num
                  label="时区"
                  path="server.timezone_offset_hours"
                  unit="小时"
                  tip="节目单与录制时间使用的时区偏移。中国标准时间为 8。"
                />
                <Pick
                  label="回看格式"
                  path="server.catchup_tpl"
                  tip="生成回看地址使用的模板。不同播放器与上游支持的占位符不同。"
                  options={CATCHUP_TEMPLATES}
                />
                <Pick
                  label="日志级别"
                  path="server.log_level"
                  tip="写入运行日志的最低级别。排查问题时可临时改为 debug。"
                  options={LOG_LEVELS.map((level) => ({ value: level, label: level }))}
                />
                <Text
                  label="首页跳转"
                  path="server.home_url"
                  tip="访问根路径 / 时的跳转地址；留空显示默认首页。"
                />
                <Text
                  label="404 跳转"
                  path="server.not_found_url"
                  tip="频道或路径不存在时跳转的地址。"
                />
                <Text
                  label="错误跳转"
                  path="server.error_url"
                  tip="上游或代理出错时跳转的地址。"
                />
              </div>
              <Lines
                label="M3U 头部"
                path="server.m3u_header_lines"
                tip="下发 M3U 时插在最前面的行，一行一条。常用于附加标记头。"
                hint="一行一条，下发订阅时插在最前"
                rows={3}
                placeholder="#EXT-X-APTV-PREVIEW: FALSE"
              />
            </Card>

            <Card
              title="功能开关"
              note="关闭后侧栏不显示该分区，相关接口返回 403"
              tip="只控制面板入口与接口可见性，不会删除已有数据。"
            >
              <div className="fields">
                <BoolPick
                  label="定时录制"
                  path="features.recording"
                  tip="显示「定时录制」分区与录制接口。"
                />
                <BoolPick
                  label="回看管理"
                  path="features.livestream"
                  tip="显示「回看管理」分区与回看接口。"
                />
                <BoolPick
                  label="台标管理"
                  path="features.logo"
                  tip="显示「台标管理」分区与台标接口。"
                />
                <BoolPick
                  label="订阅聚合"
                  path="features.playlist"
                  tip="显示「订阅聚合」分区与聚合订阅接口。"
                />
                <BoolPick
                  label="视频素材"
                  path="features.assets"
                  tip="显示「视频素材」分区与素材接口。"
                />
                <BoolPick
                  label="用户与设备"
                  path="features.users"
                  tip="显示「用户与设备」分区与账号、设备令牌接口。"
                />
              </div>
              <p className="muted">
                节目指南由「节目指南」页的总开关控制，频道热度由下方「监控」卡片的
                访问统计控制。概览、缓存状态、分组管理、系统设置、安全管控与运行日志
                始终显示。
              </p>
              <p className="muted">
                普通用户登录后只看到「我的订阅」：自己的订阅地址、EPG 地址与授权频道。
                其余分区与接口仅管理员可用。
              </p>
            </Card>

            <Card
              title="面板认证"
              note="账号在「用户与设备」分区管理"
              tip="账号、角色与设备令牌存放在 users.db，不写进配置文件。"
            >
              <p className="muted">
                面板账号、角色与设备令牌存在 <code className="mono">users.db</code>，
                不再写进配置文件。请到「用户与设备」分区新建用户或重置密码。
              </p>
            </Card>

            <Card
              title="切片伪装"
              note="把切片伪装成图片，降低被举报风险"
              tip="只改下发的扩展名，不改变切片内容。个别播放器可能不识别伪装后的地址。"
            >
              <div className="fields">
                <BoolPick
                  label="切片伪装"
                  path="cache.disguise.enabled"
                  tip="总开关。关闭时切片按原扩展名下发。"
                />
                <BoolPick
                  label="字幕 .vtt"
                  path="cache.disguise.subtitle"
                  tip="把字幕文件也伪装成图片。兼容性较差，建议关闭。"
                  hint="推荐关闭"
                />
                <BoolPick
                  label="列表伪装"
                  path="cache.disguise.playlist"
                  tip="把 m3u8 清单伪装成图片，降低特征识别。"
                  hint="推荐开启"
                />
                <BoolPick
                  label="主列表"
                  path="cache.disguise.master"
                  tip="同时伪装主清单。兼容性较差，建议关闭。"
                  hint="推荐关闭"
                />
              </div>
              <Multi
                label="伪装格式"
                path="cache.disguise.formats"
                tip="切片轮换使用的图片扩展名；关闭伪装时下发 .ts。"
                hint="多选，按切片轮换；关闭伪装时下发 .ts"
                options={['jpg', 'png', 'gif', 'bmp', 'webp']}
              />
            </Card>

            <Card
              title="监控"
              note="分组页状态点与在线人数"
              tip="健康探测决定分组页的绿点/红点；访问统计决定频道热度与在线人数。"
            >
              <div className="fields">
                <BoolPick
                  label="健康探测"
                  path="health.enabled"
                  tip="定期探测频道上游是否可用；关闭后分组页不显示状态点。"
                />
                <Num
                  label="探测周期"
                  path="health.interval_secs"
                  unit="秒"
                  tip="两次探测之间的间隔，服务内下限约 10 秒。"
                />
                <Num
                  label="探测超时"
                  path="health.timeout_secs"
                  unit="秒"
                  tip="单次探测的超时时间。"
                />
                <Num
                  label="探测并发"
                  path="health.concurrency"
                  unit="个"
                  tip="同时探测的频道数。低配机器建议 2 到 3。"
                />
                <BoolPick
                  label="访问统计"
                  path="stats.enabled"
                  tip="统计在线人数与频道热度；关闭后「频道热度」没有数据。"
                />
                <Num
                  label="在线窗口"
                  path="stats.online_window_secs"
                  unit="秒"
                  tip="窗口内出现过的播放 token 计为在线观众。"
                  hint="窗口内的播放 token 计为在线"
                />
              </div>
            </Card>

            <Card
              title="缓存配置"
              note="后端与容量"
              tip="缓存后端决定切片存放位置。容量为 0 时按推荐值或内存索引上限自动处理。"
              actions={
                <button
                  className="ghost"
                  title="删除已落盘的清单与切片"
                  disabled={busy === 'flush'}
                  onClick={() => setConfirmFlush(true)}
                >
                  {busy === 'flush' ? '清理中…' : '清空本地缓存'}
                </button>
              }
            >
              <div className="fields">
                <Pick
                  label="后端"
                  path="cache.backend"
                  tip="disk 写入磁盘，重启后保留；memory 只占内存，重启清空。"
                  options={[
                    { value: 'disk', label: '硬盘（disk）' },
                    { value: 'memory', label: '内存（memory，重启清空）' },
                  ]}
                />
                <Text
                  label="缓存目录"
                  path="cache.disk_path"
                  tip="切片落盘目录，相对进程工作目录。保存后需重启进程。"
                  hint="相对进程工作目录；改后需重启"
                />
                <Num
                  label="硬盘上限"
                  path="cache.disk_max_mb"
                  unit="MB"
                  tip="磁盘缓存上限。0 表示不限；与「磁盘回收」的上限同时非 0 时取更小值。"
                />
                <Num
                  label="内存上限"
                  path="cache.memory_max_mb"
                  unit="MB"
                  tip="内存缓存上限。0 表示不限；按每条约 256 B 估算成索引条目数。"
                />
              </div>
              <p className="muted">
                目标 v0.8.2 的这几个容量键都没生效（F-015）。本实现让上面两个真正生效；
                segment_cache_max_mb、m3u8_max_entries、segment_max_entries 保留键面但不消费。
              </p>
            </Card>

            <Card
              title="预载配置"
              note="仅 Proxy 模式生效"
              tip="预取后续切片，减少起播后的卡顿。并发过高会挤占上行带宽。"
            >
              <div className="fields">
                <BoolPick
                  label="预载"
                  path="cache.prefetch.enabled"
                  tip="播放前预取后续切片；仅 Proxy 模式生效。"
                />
                <Num
                  label="拉取并发"
                  path="cache.prefetch.concurrency"
                  unit="个"
                  tip="同时发出的预取请求数。"
                />
                <Num
                  label="拉取超时"
                  path="cache.prefetch.timeout"
                  unit="秒"
                  tip="单个预取请求的超时时间。"
                />
                <Num
                  label="HLS 预载"
                  path="cache.prefetch.prefetch_limit"
                  unit="片"
                  tip="每个清单预取的切片数。0 表示抓满清单窗口。"
                  hint="0 = 抓满清单窗口"
                />
                <Num
                  label="MPD 预载"
                  path="cache.prefetch.mpd_readahead"
                  unit="片"
                  tip="MPD 清单向前预读的切片数。"
                />
                <Num
                  label="跳过末尾"
                  path="cache.prefetch.skip_tail"
                  unit="片"
                  tip="清单末尾不预取的切片数，避免请求还没生成的切片。"
                  hint="本实现扩展：清单末尾不预取"
                />
              </div>
            </Card>

            <Card
              title="磁盘回收"
              note="1 GB 盘的硬前提"
              tip="超过上限或超龄时自动删除切片。1 GB 小盘必须开启。"
              actions={
                <button
                  className="ghost"
                  title="立即采样一次工作集，更新 CDN 推荐值"
                  disabled={busy === 'estimate'}
                  onClick={() => void estimate()}
                >
                  {busy === 'estimate' ? '估算中…' : '立即估算'}
                </button>
              }
            >
              <div className="fields">
                <BoolPick
                  label="磁盘回收"
                  path="cache.gc.enabled"
                  tip="超过上限或超龄时自动删除切片。"
                />
                <Num
                  label="占用上限"
                  path="cache.gc.max_disk_mb"
                  unit="MB"
                  tip="磁盘缓存上限。0 表示按可用空间自动推荐。"
                  hint={capHint}
                />
                <Num
                  label="超龄删除"
                  path="cache.gc.max_age_secs"
                  unit="秒"
                  tip="切片保留的最长时间，超过后删除。"
                />
                <Num
                  label="扫描周期"
                  path="cache.gc.interval_secs"
                  unit="秒"
                  tip="两次回收扫描的间隔，服务内下限 30 秒。"
                />
                <Num
                  label="单轮上限"
                  path="cache.gc.scan_limit"
                  unit="个文件"
                  tip="每轮最多检查的文件数，避免一次扫描占用太久。"
                />
                <Num
                  label="可用占比"
                  path="cache.gc.disk_budget_ratio"
                  step={0.05}
                  tip="自动上限最多占磁盘可用空间的比例，默认 0.8。"
                  hint="自动上限最多占可用空间的比例"
                />
                <Num
                  label="预留"
                  path="cache.gc.reserve_mb"
                  unit="MB"
                  tip="计算自动上限时为系统预留的空间。"
                />
                <Num
                  label="自动下限"
                  path="cache.gc.min_mb"
                  unit="MB"
                  tip="自动上限不会低于这个值。"
                />
              </div>
            </Card>

            {recommend ? (
              <div className="span-all">
                <RecommendCard recommend={recommend} />
              </div>
            ) : null}

            <Card
              title="CDN 兜底"
              note="外部 CDN 侧不生效时的服务端兜底；默认关闭"
              tip="开启后由服务端补齐缓存头与失败状态码，避免 CDN 缓存空响应。"
            >
              <div className="fields">
                <BoolPick
                  label="启用 CDN 兜底"
                  path="cdn.enabled"
                  tip="关闭时响应头与状态码与旧版逐字节一致。"
                  hint="关闭时响应头与状态码与旧版逐字节一致"
                />
                <BoolPick
                  label="被动实测"
                  path="cdn.auto_tune"
                  tip="从真实播放流量推算 TTL 与磁盘上限，无需主动压测。"
                  hint="从真实播放流量推算推荐值"
                />
                <Num
                  label="切片 max-age"
                  path="cdn.segment_max_age_secs"
                  unit="秒，0 = 自动"
                  tip="切片缓存时长。0 表示使用推荐值。"
                  hint={ttlHint}
                />
                <Num
                  label="上游失败状态码"
                  path="cdn.empty_status"
                  tip="上游返回空体时使用的状态码。502 可避免 CDN 缓存空响应。"
                  hint="默认 502：让「200 + 空体」不被 CDN 缓存"
                />
                <Text
                  label="对外基址"
                  path="cdn.public_base"
                  placeholder="https://cdn.example"
                  tip="CDN 没有回传 X-Forwarded-Proto 时使用；留空按 Host 推导。"
                  hint="CDN 不下发 X-Forwarded-Proto 时用；留空则按 Host 推导"
                />
                <Toggle
                  label="命中路径也标 public"
                  path="cdn.shared_cache_hits"
                  tip="缓存命中时也发送 public 缓存头，允许 CDN 继续缓存。"
                />
                <Toggle
                  label="空体加 no-store"
                  path="cdn.dont_cache_empty"
                  tip="上游返回空体时附加 no-store，避免 CDN 缓存空响应。"
                />
              </div>
            </Card>

            <Card
              title="切片缓存"
              note="本实现扩展：磁盘索引与内存索引上限"
              tip="这些上限只影响索引条目数，不直接限制磁盘字节。0 表示交给默认值。"
            >
              <div className="fields">
                <Num
                  label="全局条目上限"
                  path="cache.segments.global_max"
                  tip="所有频道的切片索引总上限。0 表示不限，与内存上限取更小者。"
                  hint="0 = 不限；与「内存上限」取更小者"
                />
                <Num
                  label="单频道上限"
                  path="cache.segments.per_channel_max"
                  tip="单个频道保留的切片索引数。0 表示使用内置默认 4096。"
                  hint="0 = 仅用内置默认 4096"
                />
                <Num
                  label="保留条数"
                  path="cache.segments.retain_count"
                  tip="每个清单保留的切片引用数。"
                />
              </div>
            </Card>

            <div className="span-all">
            <Card
              title="字幕设置"
              note="仅 DASH 模式频道生效"
              wide
              tip="样式参数会注入到 VTT 字幕；机翻使用已保存的配置，修改后需先保存再测试。"
              actions={
                <button
                  className="ghost"
                  title="删除已生成的字幕译文缓存"
                  disabled={busy === 'subclear'}
                  onClick={() => setConfirmSubClear(true)}
                >
                  {busy === 'subclear' ? '清理中…' : '清理译文缓存'}
                </button>
              }
            >
              <Toggle
                label="开启（下方参数生效，注入到 VTT）"
                path="subtitle.style.enabled"
                tip="把字号、颜色、位置注入 VTT 字幕；关闭时原样返回。"
                hint="关闭时原样返回字幕"
              />
              <div className="fields">
                <Pick
                  label="字号"
                  path="subtitle.style.font_size"
                  tip="字幕字号百分比，80% 为默认值。"
                  options={FONT_SIZES}
                />
                <Pick
                  label="颜色"
                  path="subtitle.style.color"
                  tip="字幕颜色；默认颜色跟随播放器。"
                  options={SUBTITLE_COLORS}
                />
                <Toggle
                  label="字体加粗"
                  path="subtitle.style.bold"
                  tip="给字幕加粗显示。"
                />
                <Range
                  label="位置"
                  path="subtitle.style.position"
                  min={0}
                  max={100}
                  unit="%"
                  tip="字幕垂直位置，0 顶部、100 底部。"
                  hint="越大越靠下"
                />
              </div>
              <Toggle
                label="字幕机翻"
                path="subtitle.translate.enabled"
                tip="把字幕翻译成目标语言；上游必须有字幕轨道。"
              />
              <Chips
                label="目标语言"
                path="subtitle.translate.targets"
                primaryPath="subtitle.translate.target"
                options={SUBTITLE_LANGS}
                tip="可多选。第一个选中的语言作为主目标语言。"
                hint="可多选；第一个选中的作为主目标语言"
              />
              <div className="fields">
                <Toggle
                  label="显示双语"
                  path="subtitle.translate.bilingual"
                  tip="同时显示原文与译文。"
                />
                <BoolPick
                  label="显示方式"
                  path="subtitle.translate.translation_first"
                  tip="译文在上还是原文在上。"
                  onLabel="译文在上"
                  offLabel="译文在下"
                />
                <Toggle
                  label="默认选中译轴"
                  path="subtitle.translate.set_default"
                  tip="播放器打开时默认选中翻译后的字幕轨。"
                />
                <Num
                  label="翻译超时"
                  path="subtitle.translate.timeout_ms"
                  unit="ms"
                  tip="单条字幕翻译的超时时间（毫秒）。"
                />
              </div>
              <div className="fields">
                <Text
                  label="MTranServer"
                  path="subtitle.translate.url"
                  placeholder="http://127.0.0.1:8989"
                  tip="自建 MTranServer 地址；留空使用内置免费机翻。"
                  hint="留空则用内置免费机翻"
                />
                <Text
                  label="API Key"
                  path="subtitle.translate.token"
                  tip="MTranServer 的访问令牌；没有就留空。"
                  hint="没填就留空"
                />
              </div>
              <div className="row">
                <button
                  className="ghost"
                  title="用已保存的配置测试机翻服务连通性"
                  disabled={busy === 'mtran'}
                  onClick={() => void testMtran()}
                >
                  {busy === 'mtran' ? '测试中…' : '测试'}
                </button>
                <span className="muted">测试用的是已保存的配置：改了地址先保存再测。</span>
              </div>
            </Card>
            </div>

            <div className="span-all">
            <Card
              title="分享（多 VPS）"
              note="主从互拉分组与订阅；对端用 ?key= 鉴权"
              wide
              tip="master 汇总下线数据，follower 从主机拉取。两端密钥必须一致。"
            >
              <div className="fields">
                <Toggle
                  label="启用分享"
                  path="share.enabled"
                  tip="开启主从互拉；关闭后不发送也不接收分享数据。"
                />
                <Pick
                  label="角色"
                  path="share.role"
                  tip="master 拉取下线；follower 从主机拉取。角色会影响安全策略。"
                  options={[
                    { value: 'master', label: 'master（主机，拉取下线）' },
                    { value: 'follower', label: 'follower（从机，从主机拉取）' },
                  ]}
                  hint="角色会影响「安全管控」的策略"
                />
                <Text
                  label="分享密钥"
                  path="share.key"
                  tip="对端用 ?key= 传入，主从两端必须一致。"
                  hint="对端用 ?key= 传入；主从必须一致"
                />
                <Num
                  label="轮询周期"
                  path="share.poll_secs"
                  unit="秒"
                  tip="follower 拉取主机的间隔，实际下限 5 秒。"
                  hint="实际下限 5 秒"
                />
                <Toggle
                  label="订阅地址带分享 token"
                  path="share.share_token"
                  tip="目标键；本实现仅回显，不改变订阅地址。"
                  hint="目标键：本实现仅回显，不改变订阅地址"
                />
              </div>
              <div className="fields">
                <Text
                  label="主机地址"
                  path="share.master_url"
                  tip="follower 从这里拉取主机数据。"
                  placeholder="https://master.example"
                />
                <Lines
                  label="下线地址"
                  path="share.follower_urls"
                  tip="master 逐个拉取这些地址上的分组与订阅。"
                  rows={2}
                  hint="一行一个；master 逐个拉取"
                />
              </div>
              <Lines
                label="允许拉取的来源"
                path="share.trust_ips"
                tip="精确 IP 或 a.b.c.0/24；留空表示不限制来源。"
                rows={2}
                hint="精确 IP 或 a.b.c.0/24；留空表示不限制"
              />
            </Card>
            </div>

            <div className="span-all">
            <Card
              title="DASH 调优"
              note="304 增量刷新、多 CDN 故障切换、断流续传、片段级容错都是自动的，无需配置"
              wide
              tip="三组参数默认值已适配大多数源。不确定用途时不要改，以免影响起播与卡顿。"
            >
              <p className="muted">
                三组参数默认值已适配绝大多数源，不确定用途就不要改——改错会直接影响起播与卡顿。
              </p>
              <details>
                <summary>播放体验（影响延迟、起播速度、卡顿稳定性）</summary>
                <div className="fields">
                  <Num
                    label="媒体窗口"
                    path="cache.dash.playlist_min_secs"
                    unit="秒"
                    step={0.5}
                    tip="媒体清单保留的时间窗。过短会让回看和追帧变难。"
                    hint="媒体清单保留的时间窗"
                  />
                  <Num
                    label="时移偏移"
                    path="cache.dash.start_time_offset_secs"
                    unit="秒"
                    step={0.5}
                    tip="清单起点相对直播时间的偏移，用于吸收上游发布延迟。"
                  />
                  <Num
                    label="发布延迟"
                    path="cache.dash.publish_delay_secs"
                    unit="秒"
                    step={0.5}
                    tip="大于 0 时压后若干段再发布，增加缓冲换取稳定。"
                    hint=">0 时压后 ceil(延迟/切片时长) 段"
                  />
                  <Num
                    label="保留下限"
                    path="cache.dash.retain_min_secs"
                    unit="秒"
                    step={0.5}
                    tip="清单至少保留的时长。"
                  />
                  <Num
                    label="额外抓取"
                    path="cache.dash.fetch_extra_segments"
                    unit="段"
                    tip="发布延迟为 0 时额外压后的段数。"
                    hint="发布延迟为 0 时压后这么多段"
                  />
                  <Num
                    label="快速起播"
                    path="cache.dash.fast_start_secs"
                    unit="秒"
                    step={0.5}
                    tip="目标侧无可观测差异，保留配置即可。"
                    hint="目标侧无可观测差异，保留配置"
                  />
                </div>
              </details>
              <details>
                <summary>网络性能（高级，默认值适用大多数场景）</summary>
                <div className="fields">
                  <Num
                    label="清单刷新下限"
                    path="cache.dash.mpd_refresh_min_secs"
                    unit="秒"
                    step={0.5}
                    tip="清单自适应刷新的最短间隔。"
                  />
                  <Num
                    label="清单刷新上限"
                    path="cache.dash.mpd_refresh_max_secs"
                    unit="秒"
                    step={0.5}
                    tip="清单自适应刷新的最长间隔。"
                  />
                  <Num
                    label="清单超时"
                    path="cache.dash.mpd_timeout_secs"
                    unit="秒"
                    tip="拉取 MPD 清单的超时时间。"
                  />
                  <Num
                    label="切片超时"
                    path="cache.dash.segment_timeout_secs"
                    unit="秒"
                    tip="拉取单个切片的超时时间。"
                  />
                  <Num
                    label="拉取并发"
                    path="cache.dash.fetch_concurrency"
                    unit="个"
                    tip="同时拉取的切片数。过高会挤占上行带宽。"
                  />
                  <Num
                    label="请求间隔下限"
                    path="cache.dash.min_request_interval_ms"
                    unit="毫秒"
                    tip="两次上游请求之间的最小间隔，避免触发限流。"
                  />
                  <Num
                    label="重试次数"
                    path="cache.dash.retry_max"
                    unit="次"
                    tip="单个请求失败后的最大重试次数。"
                  />
                  <Num
                    label="首次退避"
                    path="cache.dash.retry_initial_ms"
                    unit="毫秒"
                    tip="第一次重试前等待的时间。"
                  />
                  <Num
                    label="退避倍数"
                    path="cache.dash.retry_backoff"
                    step={0.5}
                    tip="每次重试等待时间的增长倍数。"
                  />
                  <Num
                    label="退避上限"
                    path="cache.dash.retry_max_ms"
                    unit="毫秒"
                    tip="重试等待时间的上限。"
                  />
                </div>
              </details>
              <details>
                <summary>特殊场景（高级，一般无需调整）</summary>
                <div className="fields">
                  <Num
                    label="看门狗周期"
                    path="cache.dash.watchdog_tick_secs"
                    unit="秒"
                    tip="检查会话是否卡死的周期。"
                  />
                  <Num
                    label="停滞判定"
                    path="cache.dash.watchdog_stall_secs"
                    unit="秒"
                    tip="超过这个时长没有新切片就判定为停滞。"
                  />
                  <Num
                    label="重建上限"
                    path="cache.dash.watchdog_rebuild_max"
                    unit="次"
                    tip="同一会话连续重建的最多次数。"
                  />
                  <Num
                    label="重建冷却"
                    path="cache.dash.watchdog_cooldown_secs"
                    unit="秒"
                    tip="连续失败达到上限后，冷却多久再尝试重建。"
                    hint="连续失败达上限后冷却"
                  />
                  <Num
                    label="重建错开"
                    path="cache.dash.watchdog_rebuild_stagger_ms"
                    unit="毫秒"
                    tip="多个会话重建时彼此错开的时间。"
                  />
                  <Num
                    label="清单停滞"
                    path="cache.dash.playlist_stall_secs"
                    unit="秒"
                    tip="清单多久没有更新就触发重建。0 表示使用默认策略。"
                  />
                  <Num
                    label="按需空闲回收"
                    path="cache.dash.ondemand_idle_secs"
                    unit="秒"
                    tip="按需会话空闲多久后回收。"
                  />
                  <Num
                    label="会话最长存活"
                    path="cache.dash.session_max_lifetime_secs"
                    unit="秒"
                    tip="会话的最长存活时间。0 表示不限。"
                    hint="0 = 不限"
                  />
                  <Num
                    label="尾部连续比"
                    path="cache.dash.continuous_tail_min_ratio"
                    step={0.05}
                    tip="判断尾部切片是否连续的最低比例。"
                  />
                  <Num
                    label="整文件预算"
                    path="cache.dash.whole_file_budget_mb"
                    unit="MB"
                    tip="整文件下发的内存预算；0 表示使用默认值。"
                  />
                  <Toggle
                    label="文件名混淆"
                    path="cache.dash.obfuscate_filenames"
                    tip="把下游切片文件名改成随机名，降低特征识别。"
                  />
                  <Toggle
                    label="清单正文去重"
                    path="cache.dash.mpd_body_dedup"
                    tip="清单正文未变化时复用缓存，减少上游请求。"
                  />
                </div>
                <Pick
                  label="整文件下发"
                  path="cache.dash.whole_file_serve"
                  tip="按需决定是否把整段文件直接下发。auto 由会话长度判断。"
                  options={[
                    { value: 'auto', label: '自动（auto）' },
                    { value: 'on', label: '总是' },
                    { value: 'off', label: '从不' },
                  ]}
                  hint="取值以配置为准；目标侧该键存在回读异常（S-CACHE-07）"
                />
              </details>
            </Card>
            </div>

            <div className="span-all">
              <RawCard
                draft={draft}
                onSaved={(next) => {
                  setDraft(next);
                  setBaseline(next);
                }}
              />
            </div>
          </div>
        </DraftCtx.Provider>
      )}

      <div className="settings-full">
        <SystemPage embedded />
        <BackupPage embedded onRestored={load} />
      </div>
      {node}

      {confirmReload ? (
        <Confirm
          title="放弃未保存的修改？"
          text={`当前有 ${changed} 项修改未保存。重新读取会丢弃这些修改。`}
          confirmText="放弃并重新读取"
          onCancel={() => setConfirmReload(false)}
          onConfirm={() => {
            setConfirmReload(false);
            void load();
          }}
        />
      ) : null}

      {confirmFlush ? (
        <Confirm
          title="清空本地缓存？"
          text="会删除已落盘的清单与切片。正在播放的频道可能需要重新起播。"
          confirmText="清空缓存"
          busy={busy === 'flush'}
          onCancel={() => setConfirmFlush(false)}
          onConfirm={() => {
            setConfirmFlush(false);
            void flush();
          }}
        />
      ) : null}

      {confirmSubClear ? (
        <Confirm
          title="清理译文缓存？"
          text="会删除已生成的字幕译文，下次播放时按需重新翻译。"
          confirmText="清理译文"
          busy={busy === 'subclear'}
          onCancel={() => setConfirmSubClear(false)}
          onConfirm={() => {
            setConfirmSubClear(false);
            void clearTranslated();
          }}
        />
      ) : null}
    </div>
  );
}

/** 兜底：直接编辑整份 JSON（尚未做成字段级表单的段）。 */
function RawCard({ draft, onSaved }: { draft: Cfg; onSaved: (next: Cfg) => void }) {
  const fresh = useMemo(() => JSON.stringify(draft, null, 2), [draft]);
  const [raw, setRaw] = useState<string | null>(null);
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const { ok, err } = useToast();

  // 未编辑时始终显示最新草稿；编辑后才保留用户输入。
  const text = edited && raw !== null ? raw : fresh;
  const stale = edited && raw !== null && raw !== fresh;

  const save = async () => {
    if (!edited) return;
    setBusy(true);
    try {
      const parsed = JSON.parse(text) as Cfg;
      await putConfig(parsed);
      onSaved(parsed);
      setEdited(false);
      setRaw(null);
      ok('配置已保存');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <details>
        <summary>
          <h2 style={{ display: 'inline' }}>
            其余字段（JSON）
            <Tip text="整份配置的兜底编辑器。字段卡片改动后，这里的文本会自动同步；手工编辑后需手动重新载入。" />
          </h2>
        </summary>
        <p className="muted">
          还没有字段级表单的段可以在这里改。保存同样写回整份配置，因此字段卡片的修改也会一起生效。
        </p>
        {stale ? (
          <p className="msg warn">
            字段卡片已有新修改，这里的 JSON 是旧快照。保存前请先重新载入，或确认要覆盖字段卡片的改动。
          </p>
        ) : null}
        <textarea
          rows={16}
          spellCheck={false}
          value={text}
          onChange={(event) => {
            setRaw(event.target.value);
            setEdited(true);
          }}
        />
        <div className="row" style={{ marginTop: 12 }}>
          <button
            title="把这段 JSON 写回整份配置"
            disabled={busy || !edited}
            onClick={() => void save()}
          >
            {busy ? '保存中…' : '保存 JSON'}
          </button>
          {stale ? (
            <button
              className="ghost"
              title="丢弃这里的编辑，重新读取字段卡片的最新值"
              onClick={() => {
                setEdited(false);
                setRaw(null);
              }}
            >
              重新载入当前草稿
            </button>
          ) : null}
        </div>
      </details>
    </div>
  );
}
