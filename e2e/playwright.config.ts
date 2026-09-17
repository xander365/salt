// One Playwright test proves issue #59's own goal sentence end to end: a
// real human operator can sign into Salt and run one ordinary payroll,
// against the real router, a real PostgreSQL and a real browser (issue #68).
// This config owns everything that journey needs but is not itself part of
// it: starting the built `salt-server` and the built web bundle behind one
// origin, and bootstrapping the Operator this suite signs in as.
//
// One browser only (issue #68's own Deep Instructions: "a second is added
// when a real user has a second") — Chromium, whose exact build is pinned by
// this package's own `package-lock.json`, the same way `Cargo.lock` pins a
// crate.

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, devices } from '@playwright/test';
import { SALT_SERVER_BIN } from './salt-server-bin.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const SALT_SERVER_PORT = 8080;
const WEB_PORT = 4173;

export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // One bootstrap owns one stateful journey. Retrying only the test would
  // reuse its partially-written database and turn the retry into a different
  // scenario (duplicate people or an already-created run), so failures must
  // be reported from their original attempt.
  retries: 0,
  workers: 1,
  reporter: 'list',
  globalSetup: './global-setup.ts',
  use: {
    baseURL: `http://127.0.0.1:${WEB_PORT}`,
    trace: 'retain-on-failure',
  },
  // One browser and one worker keep the two stateful, independently-arranged
  // specs deterministic against the one bootstrapped Employer. The outputs
  // spec also runs alone from a fresh database; there is deliberately no
  // Playwright project dependency between it and the original journey.
  projects: [
    {
      name: 'journey',
      testMatch: /payroll-journey\.spec\.ts/,
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'chromium',
      testMatch: /outputs\.spec\.ts/,
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: [
    {
      // `DATABASE_URL` is inherited from this process's own environment
      // (see the repository's `AGENTS.md`) — it must already name a
      // database migrated to `payroll-app`'s current schema.
      command: SALT_SERVER_BIN,
      url: `http://127.0.0.1:${SALT_SERVER_PORT}/api/health`,
      reuseExistingServer: !process.env.CI,
      // Playwright discards a webServer's output by default, so a server
      // that refuses to start fails this suite with nothing but "Timed out
      // waiting 60000ms" and no way to tell which of the two it was.
      stdout: 'pipe',
      stderr: 'pipe',
      env: {
        ...process.env,
        SALT_ENVIRONMENT: 'development',
        // Development-only: this suite talks to salt-server over plain HTTP
        // on localhost, so the session cookie's `Secure` attribute — which
        // `ServerConfig` refuses to disable in production (issue #45) —
        // has to be off here for the cookie to survive at all.
        SALT_INSECURE_COOKIES: 'true',
        SALT_BIND_ADDR: `127.0.0.1:${SALT_SERVER_PORT}`,
      },
    },
    {
      // The built bundle (issue #68's own acceptance criterion), served the
      // same way production is: one origin, with `/api` proxied to
      // salt-server (`web/vite.config.ts`'s own `preview.proxy`, the
      // `preview`-mode twin of the proxy `npm run dev` already uses) — never
      // a mocked API (Deep Instructions).
      // `--host 127.0.0.1` is not decoration: Vite's own default is the
      // name `localhost`, and a host that resolves that to `::1` first —
      // GitHub's runners do — leaves the bundle listening where the
      // `url` below, salt-server and the browser never look.
      command: `npm run preview -- --host 127.0.0.1 --port ${WEB_PORT} --strictPort`,
      cwd: path.resolve(__dirname, '../web'),
      url: `http://127.0.0.1:${WEB_PORT}`,
      reuseExistingServer: !process.env.CI,
      stdout: 'pipe',
      stderr: 'pipe',
    },
  ],
});
