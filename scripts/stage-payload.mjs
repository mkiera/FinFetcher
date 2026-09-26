import { cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function stagePayload(projectRoot = root) {
  const destination = resolve(projectRoot, 'dist/FinFetcher');
  if (relative(resolve(projectRoot), destination).replaceAll('\\', '/') !== 'dist/FinFetcher') throw new Error('Invalid payload destination.');
  if (existsSync(dirname(destination)) && lstatSync(dirname(destination)).isSymbolicLink()) throw new Error('Payload parent cannot be a symbolic link.');
  if (existsSync(destination) && lstatSync(destination).isSymbolicLink()) throw new Error('Payload destination cannot be a symbolic link.');
  const identity = JSON.parse(readFileSync(join(projectRoot, 'build_info.json'), 'utf8'));
  const executable = join(projectRoot, 'src-tauri/target/release/FinFetcher.exe');
  if (!existsSync(executable)) throw new Error('Build the release executable before staging the installer.');
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });
  cpSync(executable, join(destination, 'FinFetcher.exe'));
  for (const file of ['build_info.json', 'LICENSE']) cpSync(join(projectRoot, file), join(destination, file));
  writeFileSync(join(destination, 'version.txt'), `${identity.version}\n`);
  mkdirSync(join(destination, 'fonts'), { recursive: true });
  cpSync(join(projectRoot, 'fonts/OFL.txt'), join(destination, 'fonts/OFL.txt'));
  const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--manifest-path', join(projectRoot, 'src-tauri/Cargo.toml'), '--locked', '--format-version', '1'], {
    cwd: projectRoot, encoding: 'utf8', windowsHide: true, maxBuffer: 32 * 1024 * 1024,
  }));
  const notices = [];
  for (const dependency of metadata.packages.filter(value => value.source).sort((a, b) => a.name.localeCompare(b.name))) {
    notices.push(`${dependency.name} ${dependency.version}\nLicense: ${dependency.license || 'See included license file'}\nSource: ${dependency.repository || `https://crates.io/crates/${dependency.name}/${dependency.version}`}\n`);
    const directory = dirname(dependency.manifest_path);
    const licenses = join(destination, 'licenses', `${dependency.name}-${dependency.version}`);
    for (const file of readdirSync(directory).filter(value => /^(?:licen[sc]e|copying|copyright|notice)(?:[-.]|$)/i.test(value))) {
      mkdirSync(licenses, { recursive: true });
      cpSync(join(directory, file), join(licenses, file), { recursive: true });
    }
    if (dependency.license_file && existsSync(join(directory, dependency.license_file))) {
      mkdirSync(licenses, { recursive: true });
      cpSync(join(directory, dependency.license_file), join(licenses, 'LICENSE'), { recursive: true });
    }
  }
  writeFileSync(join(destination, 'THIRD_PARTY_LICENSES.txt'), notices.join('\n'));
  return destination;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) console.log(stagePayload());
