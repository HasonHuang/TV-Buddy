import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// 产物前缀与目标一致：/__assets/*（S-EP-16）。
// 多页入口：面板、自助解封（F-003）。
export default defineConfig({
  base: '/__assets/',
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
