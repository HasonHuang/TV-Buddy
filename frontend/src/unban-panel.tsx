/**
 * 「自助解封」分区。
 *
 * 普通用户登录后在面板内解封当前地址，保持左菜单右页面的布局。
 * 独立页 `/page/unban` 保留，供封禁期或直接打开链接时使用。
 */

import { useCallback, useEffect, useState } from 'react';

import { clearToken, getToken, UNAUTHORIZED_EVENT } from './api';
import { displayIp } from './format';
import { Icon } from './icons';
import { Loading, PageHead, useToast } from './ui';

type IpState = {
  enabled?: boolean;
  banned?: boolean;
  ip?: string;
  remaining?: number;
  used?: number;
  max_per_month?: number;
};

/** 读取当前地址的封禁状态。未登录时清除会话并回到登录卡。 */
async function fetchWhoami(): Promise<IpState> {
  const token = getToken();
  const response = await fetch('/api/unban/whoami', {
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  if (response.status === 401) {
    clearToken();
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
    throw new Error('登录已失效。');
  }
  if (!response.ok) {
    throw new Error(`读取状态失败（${response.status}）`);
  }
  return (await response.json()) as IpState;
}

export function UnbanPanelPage() {
  const { node, ok, err } = useToast();
  const [state, setState] = useState<IpState | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setState(await fetchWhoami());
      setFailure('');
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** 为当前地址自助解封。配额用完后由后端给出原因。 */
  const unban = async () => {
    setBusy(true);
    try {
      const token = getToken();
      const response = await fetch('/api/unban', {
        method: 'POST',
        headers: token ? { Authorization: `Bearer ${token}` } : {},
      });
      if (response.status === 401) {
        clearToken();
        window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
        return;
      }
      const body = (await response.json()) as {
        ok?: boolean;
        msg?: string;
        error?: string;
        remaining?: number;
      };
      if (body.ok) {
        ok(
          typeof body.remaining === 'number'
            ? `已解封。本月还可解封 ${body.remaining} 次。`
            : '已解封。',
        );
      } else {
        err(body.msg ?? body.error ?? '解封失败。');
      }
      await load();
    } catch {
      err('请求失败，请稍后重试。');
    } finally {
      setBusy(false);
    }
  };

  const enabled = state?.enabled === true;
  const banned = enabled && state?.banned === true;
  const used = state?.used ?? 0;
  const limit = state?.max_per_month ?? 3;
  const remaining = state?.remaining ?? 0;
  const exhausted = banned && remaining === 0;

  const head = (
    <PageHead
      icon="security"
      title="自助解封"
      sub="恢复当前地址的访问"
      help="解封只影响当前网络地址。每个地址 30 天内最多 3 次。成功解封才消耗次数。"
    >
      <button
        className="ghost"
        onClick={() => void load()}
        disabled={loading || busy}
        title="重新读取当前地址的封禁状态"
      >
        <Icon name="refresh" size={14} />
        {loading ? '刷新中…' : '刷新状态'}
      </button>
    </PageHead>
  );

  if (loading && state === null) {
    return (
      <>
        {head}
        <Loading />
        {node}
      </>
    );
  }

  if (state === null) {
    return (
      <>
        {head}
        <div className="card">
          <div className="unban-hero">
            <span className="unban-hero-icon danger" aria-hidden="true">
              <Icon name="close" size={20} />
            </span>
            <div className="unban-hero-text">
              <h2>读取状态失败</h2>
              <p className="muted">{failure || '请稍后重试。'}</p>
            </div>
            <button onClick={() => void load()}>重试</button>
          </div>
        </div>
        {node}
      </>
    );
  }

  if (!enabled) {
    return (
      <>
        {head}
        <div className="card">
          <div className="unban-hero">
            <span className="unban-hero-icon muted" aria-hidden="true">
              <Icon name="security" size={20} />
            </span>
            <div className="unban-hero-text">
              <h2>IP 管控未启用</h2>
              <p className="muted">管理员没有开启 IP 管控。当前没有地址会被封禁。</p>
            </div>
          </div>
          <div className="unban-facts">
            <div className="unban-fact">
              <span>当前地址</span>
              <code className="mono" title={state.ip}>
                {displayIp(state.ip)}
              </code>
            </div>
          </div>
        </div>
        {node}
      </>
    );
  }

  return (
    <>
      {head}
      <div className="card">
        <div className="unban-hero">
          <span
            className={`unban-hero-icon${banned ? ' danger' : ''}`}
            aria-hidden="true"
          >
            <Icon name="security" size={20} />
          </span>
          <div className="unban-hero-text">
            <h2>{banned ? '当前地址已被封禁' : '当前地址未被封禁'}</h2>
            <p className="muted">
              {banned
                ? exhausted
                  ? '本月自助解封次数已用完。请等待窗口结束，或联系管理员。'
                  : '提交解封后，当前地址可以立即恢复访问。'
                : '当前地址可以正常访问面板与播放地址。'}
            </p>
          </div>
          <button
            className={banned && !exhausted ? 'danger' : undefined}
            onClick={() => void unban()}
            disabled={!banned || exhausted || busy}
            title={banned ? '为当前地址提交一次解封请求' : '当前地址未被封禁'}
          >
            {busy ? '提交中…' : !banned ? '无需解封' : exhausted ? '次数已用完' : '立即解封'}
          </button>
        </div>

        <div className="unban-facts">
          <div className="unban-fact">
            <span>当前地址</span>
            <code className="mono" title={state.ip}>
              {displayIp(state.ip)}
            </code>
          </div>
          <div className="unban-fact">
            <span>本月已用</span>
            <strong>
              {used} / {limit}
            </strong>
          </div>
          <div className="unban-fact">
            <span>本月剩余</span>
            <strong>{remaining} 次</strong>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>解封规则</h2>
            <p className="muted">解封只影响当前网络地址。次数按每 30 天计算。</p>
          </div>
        </div>
        <ul className="unban-rules">
          <li>每个地址 30 天内最多自助解封 3 次。</li>
          <li>只有成功解封才消耗次数。</li>
          <li>次数用完后，请等待窗口结束或联系管理员。</li>
        </ul>
      </div>
      {node}
    </>
  );
}
