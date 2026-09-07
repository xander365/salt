// The one place `playwright.config.ts` and `global-setup.ts` both resolve
// the built `salt-server` binary they drive, so the path used to start the
// server and the path used to bootstrap through it can never disagree.

import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export const SALT_SERVER_BIN =
  process.env.SALT_SERVER_BIN ?? path.resolve(__dirname, '../target/debug/salt-server');
