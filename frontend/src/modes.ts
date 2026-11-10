/** 分组工作模式。分组管理与订阅源共用同一套说明，避免两处漂移。 */
export const MODES: { value: string; code: string; label: string; desc: string }[] = [
  { value: 'proxy', code: 'PROXY', label: '服务器转发', desc: '视频经过本机。兼容性最好。' },
  { value: 'remux', code: 'REMUX', label: '服务器转封装', desc: '本机重新打包视频流。占用 CPU。' },
  { value: 'dash', code: 'DASH', label: 'DASH 转发', desc: '处理 DASH 清单。源必须提供 MPD。' },
  { value: 'redirect', code: '302', label: '跳转源地址', desc: '播放器直接连源。会显示源地址。' },
  { value: 'rewrite', code: 'REWRITE', label: '改写地址', desc: '本机改写清单地址，播放器再连源。' },
  { value: 'passthrough', code: 'PASSTHROUGH', label: '原样直通', desc: '直接使用源地址。不经过本机。' },
];

/** 中文名加短标识，如「服务器转发（PROXY）」。名称已含标识时不重复添加。 */
export function modeText(mode: string): string {
  const item = MODES.find((entry) => entry.value === mode);
  if (!item) return mode;
  return item.label.includes(item.code) ? item.label : `${item.label}（${item.code}）`;
}
