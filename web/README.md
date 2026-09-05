# Salt — web

The browser client (Spec 3 of 3, `docs/domain/operator-auth-http-web-grill.md`).
React + TypeScript + Vite + React Router + TanStack Query. Not a Cargo
workspace member — nothing under `crates/` depends on this directory.

## Development

```sh
npm install
npm run dev
```

The dev server proxies `/api` to `salt-server` at `http://localhost:8080`
(`vite.config.ts`), so run `salt-server` separately. There is no CORS layer
in either environment — development is one origin because of this proxy,
production because Caddy serves both from one origin.

## Scripts

- `npm run dev` — Vite dev server.
- `npm run typecheck` — `tsc -b`, no emit.
- `npm run lint` — `oxlint --deny-warnings`. Every finding fails the build:
  oxlint reports most of its rules at warning severity and would otherwise
  exit zero on a real mistake, so CI would pass on broken code.
- `npm run build` — type-check, then the production bundle.

Node version is pinned in `.nvmrc`.
