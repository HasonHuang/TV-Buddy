/** 分组工作模式。分组管理与订阅源共用同一套说明，避免两处漂移。 */
export const MODES: { value: string; code: string; label: string; desc: string }[] = [
  { value: 'proxy', code: 'PROXY', label: '服务器转发', desc: '视频经过本机。兼容性最好。' },
  { value: 'remux', code: 'REMUX', label: '服务器转封装', desc: '本机重新打包视频流。占用 CPU。' },
  { value: 'dash', code: 'DASH', label: 'DASH 转发', desc: '处理 DASH 清单。源必须提供 MPD。' },
  { value: 'redirect', code: '302', label: '跳转源地址', desc: '播放器直接连源。会显示源地址。' },
  { value: 'rewrite', code: 'REWRITE', label: '改写地址', desc: '本机改写清单地址，播放器再连源。' },
  { value: 'passthrough', code: 'PASSTHROUGH', label: '原样直通', desc: '直接使用源地址。不经过本机。' },
];

/** 模式下拉选项。`exclude` 用于过滤后端不接受的值。 */
export function modeOptions(
  exclude: string[] = [],
): { value: string; label: string; desc: string }[] {
  return MODES.filter((mode) => !exclude.includes(mode.value)).map((mode) => ({
    value: mode.value,
    label: modeText(mode.value),
    desc: mode.desc,
  }));
}

/** 同步源可选的工作模式。后端拒绝同步源使用 `passthrough`（`src/config.rs`）。 */
export function syncModeOptions(): { value: string; label: string; desc: string }[] {
  return modeOptions(['passthrough']);
}

/** 中文名加短标识，如「服务器转发（PROXY）」。名称已含标识时不重复添加。 */
export function modeText(mode: string): string {
  const item = MODES.find((entry) => entry.value === mode);
  if (!item) return mode;
  return item.label.includes(item.code) ? item.label : `${item.label}（${item.code}）`;
}

/** 模式徽标的短标识，如 `PROXY`、`302`。未知模式转大写。 */
export function modeCode(mode: string): string {
  return MODES.find((item) => item.value === mode)?.code ?? mode.toUpperCase();
}

/** 徽标的悬停说明。 */
export function modeTip(mode: string): string {
  const item = MODES.find((entry) => entry.value === mode);
  return item ? `${modeText(mode)}：${item.desc}` : mode;
}
