import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Development runs on one origin: this proxies `/api` to salt-server rather
// than the browser calling a second origin directly, so no CORS layer is
// ever needed in either environment (§0.15 of
// docs/domain/operator-auth-http-web-grill.md). salt-server's own default
// bind address is 0.0.0.0:8080 (crates/salt-server/src/config.rs).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
      },
    },
  },
})
