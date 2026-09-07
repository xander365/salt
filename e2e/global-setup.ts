// Runs once before the suite, after `playwright.config.ts`'s own
// `webServer` entries are up. Creates the Operator, the Employer and the
// Owner membership through the real `salt-server bootstrap` command — no
// SQL and no test-only route (issue #68's own acceptance criterion) — and
// writes the credentials to a file the test itself reads back, since
// bootstrap is the only place they ever exist in plain text.
//
// `payroll_app::bootstrap` refuses outright once *any* Operator exists in
// the database (§0.2 — "a first-run command, not an administrative back
// door"), so `DATABASE_URL` must name a freshly migrated database with none
// yet, every time this suite runs — CI's own ephemeral Postgres service
// satisfies that by construction. The email is still randomised per run:
// it costs nothing, and it is what would matter if that rule ever loosened.

import { randomUUID } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SALT_SERVER_BIN } from './salt-server-bin.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export const CREDENTIALS_PATH = path.resolve(__dirname, '.auth/operator.json');

export interface BootstrappedOperator {
  email: string;
  password: string;
  employerName: string;
}

export default async function globalSetup(): Promise<void> {
  const operator: BootstrappedOperator = {
    email: `operator-${randomUUID()}@example.com`,
    password: 'correct horse battery staple',
    employerName: 'Acme Corp',
  };

  await new Promise<void>((resolve, reject) => {
    const child = spawn(
      SALT_SERVER_BIN,
      [
        'bootstrap',
        '--email',
        operator.email,
        '--display-name',
        'Alice Operator',
        '--employer-name',
        operator.employerName,
        // A calendar-month schedule, the same one
        // `payroll_journey.rs`'s own `an_authorized_operator` sets up —
        // §0's Deep Instructions name that Rust journey as the one this
        // browser test is a person in front of.
        '--period-end-day',
        'last-day-of-month',
      ],
      {
        // `DATABASE_URL` and everything else this process needs is
        // inherited; only `SALT_ENVIRONMENT` is bootstrap's own concern
        // (`ServerConfig::from_env` requires it), and this suite runs
        // against a development database either way.
        env: { ...process.env, SALT_ENVIRONMENT: 'development' },
        stdio: ['pipe', 'pipe', 'pipe'],
      },
    );

    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (chunk: Buffer) => {
      stdout += chunk.toString();
    });
    child.stderr.on('data', (chunk: Buffer) => {
      stderr += chunk.toString();
    });

    // Never a flag (crates/salt-server/src/main.rs's own `read_password`):
    // stdin is the only channel this password ever travels over.
    child.stdin.write(`${operator.password}\n`);
    child.stdin.end();

    child.on('error', reject);
    child.on('exit', (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(
          new Error(
            `salt-server bootstrap exited with code ${code}\nstdout: ${stdout}\nstderr: ${stderr}`,
          ),
        );
      }
    });
  });

  await mkdir(path.dirname(CREDENTIALS_PATH), { recursive: true });
  await writeFile(CREDENTIALS_PATH, JSON.stringify(operator, null, 2));
}
