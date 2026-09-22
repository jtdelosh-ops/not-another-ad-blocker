#!/usr/bin/env node
import { existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

export const HOST = 'com.naab.companion';

export function makePlan({ platform, browser = 'chrome', extensionId, binary, home, localAppData, uninstall = false }) {
  if (!['win32', 'darwin'].includes(platform)) throw new Error('Only Windows and macOS registration is implemented.');
  if (!['chrome', 'edge'].includes(browser)) throw new Error('Browser must be chrome or edge.');
  if (!/^[a-p]{32}$/.test(extensionId ?? '')) throw new Error('Supply the 32-character extension ID from the extensions page.');
  if (!binary || !path.isAbsolute(binary)) throw new Error('Supply an absolute companion binary path.');
  const directory = platform === 'win32'
    ? path.join(localAppData || path.join(home, 'AppData', 'Local'), 'NAAB', browser)
    : path.join(home, 'Library', 'Application Support', browser === 'chrome' ? 'Google/Chrome' : 'Microsoft Edge', 'NativeMessagingHosts');
  const manifestPath = path.join(directory, `${HOST}.json`);
  const registryKey = platform === 'win32'
    ? `HKCU\\Software\\${browser === 'chrome' ? 'Google\\Chrome' : 'Microsoft\\Edge'}\\NativeMessagingHosts\\${HOST}` : null;
  const manifest = { name: HOST, description: 'Not Another Ad Blocker local companion', path: binary, type: 'stdio', allowed_origins: [`chrome-extension://${extensionId}/`] };
  return { operation: uninstall ? 'unregister' : 'register', manifestPath, registryKey, manifest };
}

function registryValue(key) {
  // Query through the registry provider so an absent value can be distinguished
  // from access errors without interpreting localized reg.exe error messages.
  const command = "$ErrorActionPreference='Stop'; $p=$env:NAAB_QUERY_REGISTRY_PATH; if (!(Test-Path -LiteralPath $p)) { 'null'; exit 0 }; $k=Get-Item -LiteralPath $p; if (!($k.GetValueNames() -contains '')) { 'null'; exit 0 }; if ($k.GetValueKind('') -ne 'String') { throw 'Unexpected registration type' }; ConvertTo-Json -Compress -InputObject $k.GetValue('')";
  const result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', command], {
    encoding: 'utf8', windowsHide: true, env: { ...process.env, NAAB_QUERY_REGISTRY_PATH: key.replace(/^HKCU\\/, 'HKCU:\\') },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(result.stderr || 'Could not inspect existing registration.');
  return JSON.parse(result.stdout.trim());
}

export function applyPlan(plan) {
  const current = existsSync(plan.manifestPath) ? JSON.parse(readFileSync(plan.manifestPath, 'utf8')) : null;
  if (current && JSON.stringify(current) !== JSON.stringify(plan.manifest)) {
    throw new Error('An existing manifest differs. Unregister it using its original extension ID and binary path first.');
  }
  const registered = plan.registryKey ? registryValue(plan.registryKey) : null;
  if (registered && path.resolve(registered).toLowerCase() !== path.resolve(plan.manifestPath).toLowerCase()) {
    throw new Error('The host is registered to a different manifest; refusing to overwrite or remove it.');
  }
  if (plan.operation === 'unregister') {
    if (registered) {
      const result = spawnSync('reg.exe', ['delete', plan.registryKey, '/ve', '/f'], { encoding: 'utf8', windowsHide: true });
      if (result.error || result.status !== 0) throw new Error(result.error?.message || result.stderr || 'Registry removal failed.');
    }
    if (current) rmSync(plan.manifestPath);
    return;
  }
  if (!existsSync(plan.manifest.path)) throw new Error('The companion binary does not exist. Build it first.');
  mkdirSync(path.dirname(plan.manifestPath), { recursive: true });
  writeFileSync(plan.manifestPath, JSON.stringify(plan.manifest, null, 2) + '\n', 'utf8');
  if (plan.registryKey) {
    const result = spawnSync('reg.exe', ['add', plan.registryKey, '/ve', '/t', 'REG_SZ', '/d', plan.manifestPath, '/f'], { encoding: 'utf8', windowsHide: true });
    if (result.error || result.status !== 0) {
      if (!current) rmSync(plan.manifestPath);
      throw new Error(result.error?.message || result.stderr || 'Registry registration failed.');
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { values } = parseArgs({ options: {
      'extension-id': { type: 'string' }, binary: { type: 'string' }, browser: { type: 'string', default: 'chrome' },
      apply: { type: 'boolean', default: false }, uninstall: { type: 'boolean', default: false },
    }});
    const binary = values.binary ? path.resolve(values.binary) : '';
    const plan = makePlan({ platform: process.platform, browser: values.browser, extensionId: values['extension-id'],
      binary: existsSync(binary) ? realpathSync(binary) : binary, home: homedir(), localAppData: process.env.LOCALAPPDATA, uninstall: values.uninstall });
    if (values.apply) { applyPlan(plan); console.log(`${plan.operation} complete: ${plan.manifestPath}`); }
    else console.log(JSON.stringify({ ...plan, note: 'Preview only. Add --apply to perform this operation for the current user.' }, null, 2));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
