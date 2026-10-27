/** 录制与回看：任务 CRUD、直播开关、回看缓冲状态。 */

import { useCallback, useEffect, useState } from 'react';

import { api, getToken } from './api';
import type { Channel, Group } from './groups';
import { Badge, useToast } from './ui';

type Task = {
  id: string;
  name: string;
  group_id: string;
  channel_id: string;
  start_time: string;
  end_time: string;
  format: string;
  status: string;
  output_file: string;
  note: string;
  error: string;
  file_size: number;
  record_secs: number;
};

type TimeshiftStatus = {
  enabled: boolean;
  ready: boolean;
  channels: { group_id: string; channel_id: string; hours: number; segments: number }[];
};

/** `GET /panel/api/epg/programmes/{channel}` 的单条节目。 */
type Programme = {
  channel: string;
  disp_start: string | null;
  disp_end: string | null;
  /** 已含 `[epg].record_pad_before_secs` 的录制起点，可直接填进 `datetime-local`。 */
  rec_start: string | null;
  rec_end: string | null;
  live: boolean;
  title: string;
  desc: string;
};

/**
 * EPG 里的频道键：生成端用的是 `tvg_id`（缺省回退频道 id）的小写形式
 * （`epg::channel_targets` 的 `out_id`）。前端要按同一规则算，否则查不到节目。
 */
function epgKey(channel: Channel): string {
  const raw = channel.tvg_id ?? channel.id;
  return typeof raw === 'string' ? raw.toLowerCase() : channel.id.toLowerCase();
}

const STATUS_TONE: Record<string, string> = {
  pending: 'info',
  recording: 'warn',
  finished: 'ok',
  failed: 'err',
  stopped: 'muted',
};

function sizeText(bytes: number): string {
  if (!bytes) return '—';
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

/** 下载任务产物（带 Bearer）。 */
async function download(name: string): Promise<void> {
  const response = await fetch(`/panel/api/schedule/download/${encodeURIComponent(name)}`, {
    headers: { Authorization: `Bearer ${getToken()}` },
  });
  if (!response.ok) throw new Error(`${response.status} ${response.statusText}`);
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
}

export function SchedulePage({
  view = 'all',
}: {
  view?: 'all' | 'schedule' | 'livestream';
} = {}) {
  const [tasks, setTasks] = useState<Task[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [timeshift, setTimeshift] = useState<TimeshiftStatus | null>(null);
  const [form, setForm] = useState({
    group_id: '',
    channel_id: '',
    name: '',
    start_time: '',
    end_time: '',
    format: 'mkv',
  });
  const { node, ok, err } = useToast();
  const [programmes, setProgrammes] = useState<Programme[] | null>(null);
  const [epgNote, setEpgNote] = useState('');
  const [epgBusy, setEpgBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [nextTasks, nextGroups, nextTimeshift] = await Promise.all([
        api<Task[]>('GET', '/schedule'),
        api<Group[]>('GET', '/groups/export'),
        api<TimeshiftStatus>('GET', '/dash-timeshift/status').catch(() => null),
      ]);
      setTasks(nextTasks);
      setGroups(nextGroups);
      setTimeshift(nextTimeshift);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const channels = groups.find((group) => group.id === form.group_id)?.channels ?? [];
  const channel = channels.find((item) => item.id === form.channel_id);

  /**
   * 「从 EPG 加载」：按选中频道拉节目单。
   *
   * 取 `rec_start`/`rec_end` 而不是 `disp_start`/`disp_end`——后者是节目本身的起止，
   * 前者已经加了 `[epg].record_pad_before_secs` / `record_pad_after_secs` 的前后余量，
   * 正是录制该用的边界（与目标一致）。返回的串形如 `2026-10-25T19:00:00`，
   * 正好是 `datetime-local` 的格式。
   */
  const loadEpg = async () => {
    if (!channel) {
      err('请先选择分组与频道');
      return;
    }
    setEpgBusy(true);
    setProgrammes(null);
    setEpgNote('');
    try {
      const data = await api<{ programmes?: Programme[]; note?: string }>(
        'GET',
        `/epg/programmes/${encodeURIComponent(epgKey(channel))}`,
      );
      setProgrammes(data.programmes ?? []);
      setEpgNote(data.note ?? '');
      if ((data.programmes ?? []).length === 0) {
        err(data.note ?? '该频道暂无节目单，请先在「节目指南」采集或补映射');
      } else {
        ok(`读到 ${data.programmes?.length ?? 0} 条节目`);
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setEpgBusy(false);
    }
  };

  /** 选中一条节目 → 填起止时间与任务名（名称留空时由后端合成）。 */
  const applyProgramme = (item: Programme) => {
    if (!item.rec_start || !item.rec_end) {
      err('该节目缺少起止时间');
      return;
    }
    setForm({
      ...form,
      // `datetime-local` 只认到分钟，秒数直接截掉。
      start_time: item.rec_start.slice(0, 16),
      end_time: item.rec_end.slice(0, 16),
      name: item.title,
    });
    ok(`已填入「${item.title}」的录制时段`);
  };

  /** 用一个节目直接建任务（批量端点，目前只提交这一条）。 */
  const createFromProgramme = async (item: Programme) => {
    if (!form.group_id || !form.channel_id) {
      err('请选择分组与频道');
      return;
    }
    if (!item.rec_start || !item.rec_end) {
      err('该节目缺少起止时间');
      return;
    }
    try {
      const result = await api<{ created: number; failed: { index: number; error: string }[] }>(
        'POST',
        '/schedule/from-epg',
        {
          items: [
            {
              name: item.title,
              group_id: form.group_id,
              channel_id: form.channel_id,
              start_time: item.rec_start.slice(0, 19),
              end_time: item.rec_end.slice(0, 19),
              format: form.format,
            },
          ],
        },
      );
      if (result.created > 0) {
        ok(`已按节目创建任务「${item.title}」`);
        await load();
      } else {
        err(result.failed[0]?.error ?? '创建失败');
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const create = async () => {
    if (!form.group_id || !form.channel_id) {
      err('请选择分组与频道');
      return;
    }
    try {
      const payload = {
        ...form,
        name:
          form.name ||
          `${groups.find((group) => group.id === form.group_id)?.name ?? ''}-${form.channel_id}`,
      };
      await api('POST', '/schedule', payload);
      ok('任务已创建');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const stop = async (index: number) => {
    try {
      await api('POST', `/schedule/${index}/stop`);
      ok('已发送停止');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const remove = async (index: number) => {
    try {
      await api('DELETE', `/schedule/${index}`);
      ok('任务已删除');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const toggle = async (group: Group, channel: Channel) => {
    const action = channel.livestream ? 'stop' : 'start';
    try {
      await api('POST', `/livestream/${group.id}/${channel.id}/${action}`);
      ok(action === 'start' ? '已点亮并开始缓冲' : '已停止回看');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <>
      {view !== 'livestream' && (
        <>
          <div className="card">
            <h1>新建录制任务</h1>
            <div className="row">
              <select
                value={form.group_id}
                onChange={(event) =>
                  setForm({ ...form, group_id: event.target.value, channel_id: '' })
                }
              >
                <option value="">选择分组</option>
                {groups.map((group) => (
                  <option key={group.id} value={group.id}>
                    {group.name}（{group.id}）
                  </option>
                ))}
              </select>
              <select
                value={form.channel_id}
                onChange={(event) => setForm({ ...form, channel_id: event.target.value })}
              >
                <option value="">选择频道</option>
                {channels.map((channel) => (
                  <option key={channel.id} value={channel.id}>
                    {channel.name}
                  </option>
                ))}
              </select>
              <input
                placeholder="任务名（可空）"
                value={form.name}
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </div>
            <div className="row">
              <input
                type="datetime-local"
                value={form.start_time}
                onChange={(event) => setForm({ ...form, start_time: event.target.value })}
              />
              <input
                type="datetime-local"
                value={form.end_time}
                onChange={(event) => setForm({ ...form, end_time: event.target.value })}
              />
              <select
                value={form.format}
                onChange={(event) => setForm({ ...form, format: event.target.value })}
              >
                <option value="mkv">MKV（需 ffmpeg）</option>
                <option value="ts">TS</option>
              </select>
              <button onClick={() => void create()}>创建任务</button>
              <button
                className="ghost"
                disabled={!form.channel_id || epgBusy}
                onClick={() => void loadEpg()}
              >
                {epgBusy ? '读取中…' : '从 EPG 加载节目'}
              </button>
            </div>
            {programmes !== null && (
              <div className="field">
                <label>节目单（点一条填入时段，或直接排期）</label>
                {programmes.length === 0 ? (
                  <p className="muted">{epgNote || '该频道暂无节目单。'}</p>
                ) : (
                  <table className="table">
                    <thead>
                      <tr>
                        <th>时段</th>
                        <th>节目</th>
                        <th className="right">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {programmes.map((item) => (
                        <tr key={`${item.disp_start}-${item.title}`}>
                          <td className="mono nowrap">
                            {item.live && <Badge tone="ok">正在播</Badge>}{' '}
                            {(item.rec_start ?? '—').replace('T', ' ')} →{' '}
                            {(item.rec_end ?? '—').slice(11)}
                          </td>
                          <td>
                            {item.title}
                            {item.desc && <div className="muted ellipsis">{item.desc}</div>}
                          </td>
                          <td className="right nowrap">
                            <button className="ghost" onClick={() => applyProgramme(item)}>
                              填入
                            </button>
                            <button className="ghost" onClick={() => void createFromProgramme(item)}>
                              排期
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            )}
          </div>

          <div className="card">
            <h1>录制任务</h1>
            <table>
              <thead>
                <tr>
                  <th>名称</th>
                  <th>频道</th>
                  <th>时间</th>
                  <th>状态</th>
                  <th>产物</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {tasks.map((task, index) => (
                  <tr key={task.id}>
                    <td>{task.name}</td>
                    <td>
                      {task.group_id}/{task.channel_id}
                    </td>
                    <td className="muted">
                      {task.start_time} → {task.end_time}
                    </td>
                    <td>
                      <Badge tone={STATUS_TONE[task.status] ?? 'muted'}>{task.status}</Badge>
                      {(task.error || task.note) && (
                        <div className="muted">{task.error || task.note}</div>
                      )}
                    </td>
                    <td>
                      {task.output_file ? (
                        <button
                          className="ghost"
                          onClick={() => void download(task.output_file).catch(() => err('下载失败'))}
                        >
                          {sizeText(task.file_size)}
                        </button>
                      ) : (
                        '—'
                      )}
                    </td>
                    <td>
                      {task.status === 'recording' && (
                        <button className="ghost" onClick={() => void stop(index)}>
                          停止
                        </button>
                      )}
                      <button className="icon-btn" onClick={() => void remove(index)}>
                        删除
                      </button>
                    </td>
                  </tr>
                ))}
                {tasks.length === 0 && (
                  <tr>
                    <td colSpan={6} className="muted">
                      暂无任务。
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </>
      )}

      {view !== 'schedule' && (
        <div className="card">
          <h1>直播回看</h1>
          <p className="muted">
            点亮后按频道 `timeshift_hours` 缓冲；回看清单用 `?start&end` 或 `?playseek` 请求入口。
            {timeshift &&
              ` 已点亮 ${timeshift.channels.length} 个频道，已缓冲切片 ${timeshift.channels.reduce(
                (sum, channel) => sum + channel.segments,
                0,
              )} 条。`}
          </p>
          <table>
            <thead>
              <tr>
                <th>分组</th>
                <th>频道</th>
                <th>缓冲</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {groups.flatMap((group) =>
                (group.channels ?? []).map((channel) => {
                  const state = timeshift?.channels.find(
                    (item) => item.group_id === group.id && item.channel_id === channel.id,
                  );
                  return (
                    <tr key={`${group.id}/${channel.id}`}>
                      <td>{group.name}</td>
                      <td>{channel.name}</td>
                      <td>
                        {channel.livestream ? (
                          <Badge tone="ok">{state ? `${state.segments} 片` : '缓冲中'}</Badge>
                        ) : (
                          <Badge tone="muted">未点亮</Badge>
                        )}
                      </td>
                      <td>
                        <button className="ghost" onClick={() => void toggle(group, channel)}>
                          {channel.livestream ? '停止' : '点亮'}
                        </button>
                      </td>
                    </tr>
                  );
                }),
              )}
            </tbody>
          </table>
        </div>
      )}

      {node}
    </>
  );
}

/** 独立回看管理分区。 */
export function LivestreamPage() {
  return <SchedulePage view="livestream" />;
}
