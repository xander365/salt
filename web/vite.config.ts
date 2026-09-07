import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Development runs on one origin: this proxies `/api` to salt-server rather
// than the browser calling a second origin directly, so no CORS layer is
// ever needed in either environment (§0.15 of
// docs/domain/operator-auth-http-web-grill.md). salt-server's own default
// bind address is 0.0.0.0:8080 (crates/salt-server/src/config.rs) — IPv4,
// which is why the target below names `127.0.0.1` and never `localhost`: a
// machine that resolves that name to `::1` first would send every proxied
// `/api` call somewhere nothing is listening.
const apiProxy = {
  '/api': {
    target: 'http://127.0.0.1:8080',
    changeOrigin: true,
  },
}

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: apiProxy,
  },
  // `vite preview` serves this project's own build output (`dist/`) rather
  // than source, which is what `e2e`'s browser test (issue #68) drives as
  // "the built bundle" — the same one-origin shape production has behind
  // Caddy, stood up here with no Caddy of its own.
  preview: {
    proxy: apiProxy,
  },
})
