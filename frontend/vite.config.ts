import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// 产物前缀与目标一致：/__assets/*（S-EP-16）。
// 多页入口：面板、节目单、台标库、自助解封（F-003）。
export default defineConfig({
  base: '/__assets/',
  plugins: [react()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        panel: 'panel.html',
        epg: 'epg.html',
        logo: 'logo.html',
        unban: 'unban.html',
      },
    },
  },
});
