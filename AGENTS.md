# salt

## Agent skills

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
