import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: {
      // WebSocket bridge to the daemon. `changeOrigin` rewrites the Host
      // header to 127.0.0.1:3100 so the daemon's Host allowlist passes; the
      // daemon's Origin allowlist is configured to accept http://localhost:5173.
      '/ws': {
        target: 'ws://127.0.0.1:3100',
        ws: true,
        changeOrigin: true,
      },
      '/api': {
        target: 'http://127.0.0.1:3100',
        changeOrigin: true,
      },
    },
  },
})
