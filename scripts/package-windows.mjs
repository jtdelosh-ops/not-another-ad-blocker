// Builder-only script. The extracted package needs only Windows PowerShell.
import { cp, mkdir, mkdtemp, readFile, readdir, lstat, rm, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { nativeTransport } from '../tests/native-transport.mjs';

export function importedLibraries(bytes) {
  if (bytes.toString('ascii', 0, 2) !== 'MZ') throw new Error('Not a Windows executable');
  const pe = bytes.readUInt32LE(0x3c);
  if (bytes.toString('ascii', pe, pe + 4) !== 'PE\0\0' || bytes.readUInt16LE(pe + 4) !== 0x8664) throw new Error('Expected an x64 Windows executable');
  const optional = pe + 24;
  if (bytes.readUInt16LE(optional) !== 0x20b) throw new Error('Expected PE32+');
  const count = bytes.readUInt16LE(pe + 6);
  const sections = optional + bytes.readUInt16LE(pe + 20);
  function offset(rva) {
    for (let index = 0; index < count; index++) {
      const section = sections + index * 40;
      const virtual = bytes.readUInt32LE(section + 12);
      const rawSize = bytes.readUInt32LE(section + 16);
      if (rva >= virtual && rva < virtual + rawSize) return bytes.readUInt32LE(section + 20) + rva - virtual;
    }
    throw new Error(`Unmapped PE address ${rva}`);
  }
  if (bytes.readUInt32LE(optional + 112 + 13 * 8)) throw new Error('Unexpected delayed DLL imports');
  const imports = bytes.readUInt32LE(optional + 120);
  if (!imports) throw new Error('Executable has no import table');
  const names = [];
  let descriptor = offset(imports);
  for (let index = 0; index < 128; index++, descriptor += 20) {
    if (bytes.subarray(descriptor, descriptor + 20).every(value => value === 0)) return names;
    const nameOffset = offset(bytes.readUInt32LE(descriptor + 12));
    const end = bytes.indexOf(0, nameOffset);
    if (end < nameOffset || end - nameOffset > 256) throw new Error('Invalid DLL import name');
    names.push(bytes.toString('ascii', nameOffset, end).toLowerCase());
  }
  throw new Error('Unexpectedly large import table');
}

async function main() {
  if (process.platform !== 'win32' || process.arch !== 'x64') throw new Error('Package this x64 preview on an x64 Windows builder');
  const { values } = parseArgs({ options: { output: { type: 'string', default: 'companion/target/windows-preview' }, 'allow-dirty': { type: 'boolean', default: false } } });
  const root = fileURLToPath(new URL('..', import.meta.url));
  const command = (program, args, options = {}) => execFileSync(program, args, { cwd: root, encoding: 'utf8', windowsHide: true, ...options }).trim();
  const sourceCommit = command('git', ['rev-parse', 'HEAD']);
  const packageInputs = ['scripts/package-windows.mjs', 'scripts/install-windows.ps1', 'scripts/install-windows.cmd', 'scripts/uninstall-windows.cmd', 'docs/browser-preview.md'];
  let sourceDirty = command('git', ['status', '--porcelain', '--untracked-files=no']) !== '';
  try { command('git', ['ls-files', '--error-unmatch', '--', ...packageInputs], { stdio: ['ignore', 'pipe', 'pipe'] }); }
  catch { sourceDirty = true; }
  if (sourceDirty && (!values['allow-dirty'] || process.env.GITHUB_ACTIONS)) throw new Error('Commit tracked changes before packaging; --allow-dirty is for local checks only');
  if (process.env.GITHUB_SHA && process.env.GITHUB_SHA !== sourceCommit) throw new Error('Workflow and checkout commits differ');
  const output = path.resolve(root, values.output);
  await mkdir(output, { recursive: true });
  const staging = await mkdtemp(path.join(output, '.naab-package-'));
  const folder = path.join(staging, 'NAAB-Windows-Preview');
  try {
    await mkdir(path.join(folder, 'companion'), { recursive: true });
    await cp(path.join(root, 'extension/dist'), path.join(folder, 'extension'), { recursive: true });
    await cp(path.join(root, 'companion/target/release/naab-companion.exe'), path.join(folder, 'companion/naab-companion.exe'));
    for (const [source, target] of [
      ['scripts/install-windows.ps1', 'Install.ps1'], ['scripts/install-windows.cmd', 'Install.cmd'],
      ['scripts/uninstall-windows.cmd', 'Uninstall.cmd'], ['docs/browser-preview.md', 'README.md'],
    ]) await cp(path.join(root, source), path.join(folder, target));
    const binary = path.join(folder, 'companion/naab-companion.exe');
    const imports = importedLibraries(await readFile(binary));
    const system = /^(?:kernel32|kernelbase|advapi32|ntdll|bcrypt|bcryptprimitives|crypt32|ncrypt|secur32|ws2_32|userenv|user32|shell32|ole32|oleaut32|iphlpapi|normaliz|mswsock|psapi|netapi32|dbghelp|synchronization|ucrtbase)\.dll$|^(?:api-ms-win-|ext-ms-win-)[a-z0-9-]+\.dll$/;
    if (imports.some(name => !system.test(name))) throw new Error(`Non-system runtime dependency: ${imports.filter(name => !system.test(name)).join(', ')}. Build with static CRT.`);
    const metadata = JSON.parse(command('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1', '--manifest-path', 'companion/Cargo.toml']));
    const version = metadata.packages.find(item => item.name === 'naab-companion').version;
    const status = await new NativeClient(nativeTransport(binary)).status();
    if (status.companionVersion !== version) throw new Error('Companion build version is stale');
    console.log(command(process.execPath, ['--test', 'tests/native-integration.test.mjs'], { env: { ...process.env, NAAB_BINARY: binary }, stdio: ['ignore', 'pipe', 'pipe'] }));
    const extension = JSON.parse(await readFile(path.join(folder, 'extension/manifest.json'), 'utf8'));
    if (extension.version !== JSON.parse(await readFile(path.join(root, 'extension/package.json'), 'utf8')).version) throw new Error('Extension build version is stale');
    const files = [];
    async function walk(directory) {
      for (const name of (await readdir(directory)).sort()) {
        const full = path.join(directory, name);
        const stat = await lstat(full);
        if (stat.isSymbolicLink()) throw new Error('Package symlinks are not allowed');
        if (stat.isDirectory()) await walk(full);
        else if (stat.isFile()) files.push({ path: path.relative(folder, full).split(path.sep).join('/'), bytes: stat.size, sha256: createHash('sha256').update(await readFile(full)).digest('hex') });
        else throw new Error('Unexpected package entry');
      }
    }
    await walk(folder);
    await writeFile(path.join(folder, 'BUILD-INFO.json'), JSON.stringify({
      formatVersion: 1, product: 'Not Another Ad Blocker Windows browser preview',
      extensionVersion: extension.version, companionVersion: version, companionProtocolVersion: status.protocolVersion,
      sourceCommit, sourceDirty, sourceRef: process.env.GITHUB_REF_NAME ?? command('git', ['branch', '--show-current']),
      builtAt: new Date().toISOString(), target: { architecture: 'x64', importedLibraries: imports, signing: 'unsigned developer preview' },
      builder: { windows: command('powershell.exe', ['-NoProfile', '-Command', '[Environment]::OSVersion.Version.ToString()']), rust: command('rustc', ['--version']), node: process.version }, files,
    }, null, 2) + '\n');
    const filename = `NAAB-Windows-Preview-${extension.version}-x64-${sourceCommit.slice(0, 12)}${sourceDirty ? '-worktree' : ''}.zip`;
    const archive = path.join(output, filename);
    // Paths are passed as environment values, never interpolated into shell code.
    command('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', "$ErrorActionPreference='Stop'; Compress-Archive -LiteralPath $env:NAAB_ZIP_SOURCE -DestinationPath $env:NAAB_ZIP_TARGET -Force"], { env: { ...process.env, NAAB_ZIP_SOURCE: folder, NAAB_ZIP_TARGET: archive } });
    const checksum = `${createHash('sha256').update(await readFile(archive)).digest('hex')}  ${filename}\n`;
    await writeFile(path.join(output, 'SHA256SUMS.txt'), checksum);
    console.log(`Created ${archive}\n${checksum}`);
  } finally {
    if (path.dirname(staging) !== output || !path.basename(staging).startsWith('.naab-package-')) throw new Error('Unsafe package cleanup target');
    await rm(staging, { recursive: true, force: true });
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
