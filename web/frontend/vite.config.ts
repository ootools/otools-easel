import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  base: './',  // 相对路径,插件 webview 直接加载 dist
  build: {
    outDir: '../../dist',
    emptyOutDir: true,
  },
})
