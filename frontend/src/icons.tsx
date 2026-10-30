/** 面板图标集。
 *
 * 全部是 16×16 的描边图形，用 `currentColor` 取色，随文字颜色变化。
 * 只在界面里用；不引入第三方图标库，避免打包体积与许可问题。
 */

import type { ReactNode } from 'react';

export type IconName =
  | 'overview'
  | 'cache'
  | 'groups'
  | 'settings'
  | 'security'
  | 'stats'
  | 'schedule'
  | 'epg'
  | 'logo'
  | 'subs'
  | 'livestream'
  | 'assets'
  | 'logs'
  | 'tv'
  | 'plus'
  | 'edit'
  | 'trash'
  | 'download'
  | 'upload'
  | 'link'
  | 'copy'
  | 'more'
  | 'up'
  | 'down'
  | 'search'
  | 'refresh'
  | 'check'
  | 'close'
  | 'folder'
  | 'grip'
  | 'play'
  | 'image'
  | 'file'
  | 'users';

const PATHS: Record<IconName, ReactNode> = {
  overview: <path d="M2.2 13.4V8.4M6.2 13.4V5.2M10.2 13.4V9.6M14 13.4V3.2" />,
  cache: (
    <>
      <ellipse cx="8" cy="3.6" rx="5" ry="1.9" />
      <path d="M3 3.6v4.1c0 1.05 2.24 1.9 5 1.9s5-.85 5-1.9V3.6" />
      <path d="M3 7.7v4.1c0 1.05 2.24 1.9 5 1.9s5-.85 5-1.9V7.7" />
    </>
  ),
  groups: (
    <path d="M2 4.6A1.6 1.6 0 0 1 3.6 3h2.5l1.4 1.6h4.9A1.6 1.6 0 0 1 14 6.2v5.2a1.6 1.6 0 0 1-1.6 1.6H3.6A1.6 1.6 0 0 1 2 11.4z" />
  ),
  settings: (
    <>
      <circle cx="8" cy="8" r="2.1" />
      <path d="M8 1.8v1.7M8 12.5v1.7M14.2 8h-1.7M3.5 8H1.8M12.4 3.6l-1.2 1.2M4.8 11.2l-1.2 1.2M12.4 12.4l-1.2-1.2M4.8 4.8 3.6 3.6" />
    </>
  ),
  security: (
    <path d="M8 1.9 3.3 3.6v4.1c0 2.8 1.9 5.2 4.7 6.4 2.8-1.2 4.7-3.6 4.7-6.4V3.6z" />
  ),
  stats: (
    <path d="M8 13.8c2.35 0 3.9-1.55 3.9-3.6 0-2.5-2.3-3.5-2.3-5.9 0-.7.15-1.35.45-1.9C7.5 3.1 5.7 5.2 5.7 7.3c0 .75.2 1.35.45 1.85-.95.3-1.9 1.35-1.9 2.9 0 1.25 1.5 1.75 3.75 1.75z" />
  ),
  schedule: (
    <>
      <circle cx="8" cy="8" r="5.9" />
      <path d="M8 4.7V8l2.2 1.35" />
    </>
  ),
  epg: (
    <>
      <rect x="2" y="3.4" width="12" height="10.2" rx="1.6" />
      <path d="M2 6.6h12M5.6 1.9v3M10.4 1.9v3" />
    </>
  ),
  logo: (
    <>
      <rect x="2" y="3.2" width="12" height="9.6" rx="1.6" />
      <circle cx="5.9" cy="6.5" r="1.05" />
      <path d="m2.6 11.4 3.3-3 2.4 2.1 2-1.7 3.1 2.7" />
    </>
  ),
  subs: (
    <>
      <path d="M8 2 2 4.9l6 2.9 6-2.9z" />
      <path d="m2 8.2 6 2.9 6-2.9" />
      <path d="m2 11.2 6 2.9 6-2.9" />
    </>
  ),
  livestream: (
    <>
      <circle cx="8" cy="8" r="5.9" />
      <path d="M6.6 5.6 10.6 8l-4 2.4z" />
    </>
  ),
  assets: (
    <>
      <rect x="2" y="3" width="12" height="10" rx="1.4" />
      <path d="M5.4 3v10M10.6 3v10M2 8h12" />
    </>
  ),
  logs: (
    <>
      <path d="M4 1.9h4.9l3.1 3.1v9.1H4z" />
      <path d="M8.9 1.9v3.1h3.1M6.1 8.2h3.8M6.1 10.7h3.8" />
    </>
  ),
  tv: (
    <>
      <rect x="1.6" y="4.8" width="12.8" height="8.8" rx="1.9" />
      <path d="m5 1.9 3 2.4 3-2.4" />
    </>
  ),
  plus: <path d="M8 3.3v9.4M3.3 8h9.4" />,
  edit: <path d="M11.1 2.4 13.6 4.9 5.6 12.9l-3.2.7.7-3.2z" />,
  trash: <path d="M2.9 4.4h10.2M6.5 4.4V2.9h3v1.5M4.3 4.4l.7 9h6l.7-9" />,
  download: <path d="M8 2.5v7.2M5.1 7 8 9.9 10.9 7M2.9 12.6h10.2" />,
  upload: <path d="M8 10.5V2.9M5.1 5.8 8 2.9l2.9 2.9M2.9 12.6h10.2" />,
  link: (
    <>
      <path d="M6.7 9.3a2.55 2.55 0 0 0 3.6 0l1.85-1.85a2.55 2.55 0 0 0-3.6-3.6l-.85.85" />
      <path d="M9.3 6.7a2.55 2.55 0 0 0-3.6 0L3.85 8.55a2.55 2.55 0 0 0 3.6 3.6l.85-.85" />
    </>
  ),
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8.1" height="8.1" rx="1.4" />
      <path d="M10.5 5.5V3.9a1.4 1.4 0 0 0-1.4-1.4H3.9a1.4 1.4 0 0 0-1.4 1.4v5.2a1.4 1.4 0 0 0 1.4 1.4h1.6" />
    </>
  ),
  users: (
    <>
      <circle cx="6" cy="5.6" r="2.2" />
      <path d="M2.3 13.2c0-2.1 1.6-3.4 3.7-3.4s3.7 1.3 3.7 3.4" />
      <circle cx="11.5" cy="6.5" r="1.7" />
      <path d="M10.7 9.7c2 0 3.3 1.2 3.3 3.1" />
    </>
  ),

  more: (
    <>
      <circle cx="3.4" cy="8" r="1.05" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="1.05" fill="currentColor" stroke="none" />
      <circle cx="12.6" cy="8" r="1.05" fill="currentColor" stroke="none" />
    </>
  ),
  up: <path d="m4.7 9.7 3.3-3.4 3.3 3.4" />,
  down: <path d="m4.7 6.3 3.3 3.4 3.3-3.4" />,
  search: (
    <>
      <circle cx="7.2" cy="7.2" r="4.35" />
      <path d="m10.5 10.5 2.9 2.9" />
    </>
  ),
  refresh: (
    <>
      <path d="M13.3 8a5.3 5.3 0 1 1-1.55-3.75" />
      <path d="M13.5 2.7v3.1h-3.1" />
    </>
  ),
  check: <path d="m3.4 8.4 2.9 2.9 6.3-6.7" />,
  close: <path d="m4.3 4.3 7.4 7.4M11.7 4.3 4.3 11.7" />,
  folder: (
    <path d="M2 4.6A1.6 1.6 0 0 1 3.6 3h2.5l1.4 1.6h4.9A1.6 1.6 0 0 1 14 6.2v5.2a1.6 1.6 0 0 1-1.6 1.6H3.6A1.6 1.6 0 0 1 2 11.4z" />
  ),
  grip: (
    <>
      {[3.2, 8, 12.8].map((y) => (
        <g key={y}>
          <circle cx="6" cy={y} r="0.95" fill="currentColor" stroke="none" />
          <circle cx="10" cy={y} r="0.95" fill="currentColor" stroke="none" />
        </g>
      ))}
    </>
  ),
  play: <path d="M5.6 3.9 12 8l-6.4 4.1z" />,
  image: (
    <>
      <rect x="2" y="3.2" width="12" height="9.6" rx="1.6" />
      <circle cx="5.9" cy="6.5" r="1.05" />
      <path d="m2.6 11.4 3.3-3 2.4 2.1 2-1.7 3.1 2.7" />
    </>
  ),
  file: (
    <>
      <path d="M4 1.9h4.9l3.1 3.1v9.1H4z" />
      <path d="M8.9 1.9v3.1h3.1" />
    </>
  ),
};

export function Icon({
  name,
  size = 16,
  className,
}: {
  name: IconName;
  size?: number;
  className?: string;
}) {
  return (
    <svg
      className={className ? `icon ${className}` : 'icon'}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}
