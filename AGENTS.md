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
