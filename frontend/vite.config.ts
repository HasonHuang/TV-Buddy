import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// 产物前缀与新路径一致：/static/panel/*（S-EP-16）。
// 多页入口：面板、自助解封（F-003）。
export default defineConfig({
  base: '/static/panel/',
  plugins: [react()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      input: {
        panel: 'panel.html',
        unban: 'unban.html',
      },
    },
  },
});
