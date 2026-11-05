import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

// OTools 插件以相对路径加载 dist/index.html,base 必须是 './'。
export default defineConfig({
  base: './',
  plugins: [vue()],
  build: {
    target: 'chrome110',
    chunkSizeWarningLimit: 2048,
  },
  server: {
    port: 6493,
    strictPort: true,
  },
});
