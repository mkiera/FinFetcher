import { cpSync, existsSync, lstatSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const frontendFiles = ['index.html', 'style.css', 'script.js', 'desktop.js', 'icon.ico', 'icon.png', 'fonts/OFL.txt'];

export function buildFrontend(projectRoot = root) {
  const destination = resolve(projectRoot, 'dist-frontend');
  if (relative(resolve(projectRoot), destination) !== 'dist-frontend') throw new Error('Invalid frontend destination.');
  if (existsSync(destination) && lstatSync(destination).isSymbolicLink()) throw new Error('Frontend destination cannot be a symbolic link.');
  const fonts = readdirSync(resolve(projectRoot, 'fonts'), { withFileTypes: true })
    .filter(entry => entry.isFile() && /\.(?:woff2?|ttf|otf)$/i.test(entry.name)).map(entry => `fonts/${entry.name}`);
  const files = [...frontendFiles, ...fonts];
  for (const name of files) {
    if (!lstatSync(resolve(projectRoot, name)).isFile()) throw new Error(`Missing frontend file: ${name}`);
  }
  rmSync(destination, { recursive: true, force: true });
  for (const name of files) {
    const target = resolve(destination, name);
    mkdirSync(dirname(target), { recursive: true });
    cpSync(resolve(projectRoot, name), target);
  }
  return files;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) buildFrontend();
