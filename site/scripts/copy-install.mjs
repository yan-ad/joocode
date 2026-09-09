import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..', '..');
const destDir = resolve(root, 'site', 'public');

const copies = [
  ['install.bash', 'install'],
  ['install.ps1', 'install.ps1'],
  ['uninstall.sh', 'uninstall'],
];

mkdirSync(destDir, { recursive: true });
for (const [src, dest] of copies) {
  writeFileSync(resolve(destDir, dest), readFileSync(resolve(root, src), 'utf8'));
  console.log(`copied ${src} -> site/public/${dest}`);
}
