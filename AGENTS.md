# salt

## Agent skills

use /impeccable skill for any frontend work

### Issue tracker

Issues live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage labels, using their default names. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Local test database

The local PostgreSQL server accepts peer authentication for the OS user `alex`, who is a superuser. Run SQLx integration tests with:

```sh
DATABASE_URL='postgresql:///postgres?user=alex' cargo test --locked
```

Do not use the CI Docker URL (`postgres://postgres:postgres@localhost:5432/postgres`) locally; its password authentication fails against this server.

`salt-server` has no `sqlx` (ADR-0018), so its own tests cannot use `#[sqlx::test]`'s disposable per-test database — they connect directly to whatever `DATABASE_URL` names, which must therefore already be migrated to `payroll-app`'s current schema. One database can serve both suites: `payroll-app`'s `#[sqlx::test]` only uses the connection to `CREATE DATABASE` its own ephemeral copies, so it does not mind the target already holding tables. Set up once with:

```sh
createdb -h /var/run/postgresql -U alex salt_server_test
sqlx migrate run --source crates/payroll-app/migrations \
    --database-url "postgres:///salt_server_test?host=/var/run/postgresql&user=alex"
```

then run the whole workspace's tests with:

```sh
DATABASE_URL="postgres:///salt_server_test?host=/var/run/postgresql&user=alex" cargo test --workspace --locked
```

Re-run the `sqlx migrate run` command above after any migration is added to `crates/payroll-app/migrations`.

### Browser journey suite

`e2e/` holds one Playwright test (issue #68): the whole payroll journey, in
a real browser, against a built `salt-server`, the built `web/` bundle and a
real PostgreSQL. It is outside both the Cargo workspace and `web/`.

`payroll_app::bootstrap` refuses once any Operator exists, so this suite
needs a database with none — never the shared `salt_server_test` one, and a
fresh one every run:

```sh
dropdb -h /var/run/postgresql -U alex --if-exists salt_e2e
createdb -h /var/run/postgresql -U alex salt_e2e
sqlx migrate run --source crates/payroll-app/migrations \
    --database-url "postgres:///salt_e2e?host=/var/run/postgresql&user=alex"
```

Then build what the browser drives — the test starts these two servers
itself, but builds neither — and run it:

```sh
cargo build --locked -p salt-server
npm --prefix web ci && npm --prefix web run build
npm --prefix e2e ci
(cd e2e && npx playwright install chromium)
DATABASE_URL="postgres:///salt_e2e?host=/var/run/postgresql&user=alex" \
    npm --prefix e2e test
```

Re-run the `createdb`/`migrate` pair before each run, and rebuild after any
change to `salt-server` or `web/` — the suite drives the built artefacts, not
the sources.
