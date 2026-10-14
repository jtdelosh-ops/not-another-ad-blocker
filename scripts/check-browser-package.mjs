import { readFile, readdir, lstat } from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';

const directory = path.resolve(process.argv[2] ?? '');
if (!process.argv[2]) throw new Error('Pass the extracted package folder');
const info = JSON.parse(await readFile(path.join(directory, 'BUILD-INFO.json'), 'utf8'));
assert.equal(info.formatVersion, 1);
assert.match(info.sourceCommit, /^[a-f0-9]{40}$/);
if (process.env.GITHUB_SHA) { assert.equal(info.sourceCommit, process.env.GITHUB_SHA); assert.equal(info.sourceDirty, false); }
const declared = new Map(info.files.map(file => [file.path, file]));
assert.equal(declared.size, info.files.length, 'Duplicate file metadata');
const actual = [];
async function walk(folder) {
  for (const name of await readdir(folder)) {
    const full = path.join(folder, name);
    const stat = await lstat(full);
    assert.equal(stat.isSymbolicLink(), false, 'Package symlink');
    if (stat.isDirectory()) await walk(full);
    else {
      assert.equal(stat.isFile(), true);
      const relative = path.relative(directory, full).split(path.sep).join('/');
      if (relative === 'BUILD-INFO.json') continue;
      assert.doesNotMatch(relative, /naab-dns|dns-recovery|node_modules/i, 'Unexpected DNS or development runtime in package');
      const metadata = declared.get(relative);
      assert.ok(metadata, `Missing metadata for ${relative}`);
      assert.equal(metadata.bytes, stat.size);
      assert.equal(metadata.sha256, createHash('sha256').update(await readFile(full)).digest('hex'), `Checksum mismatch: ${relative}`);
      actual.push(relative);
    }
  }
}
await walk(directory);
assert.deepEqual(actual.sort(), [...declared.keys()].sort(), 'Metadata lists missing files');
const extension = JSON.parse(await readFile(path.join(directory, 'extension/manifest.json'), 'utf8'));
assert.equal(extension.version, info.extensionVersion);
const windows = info.target.architecture === 'x64';
assert.ok(windows || info.target.architecture === 'x86_64');
for (const name of windows ? ['Install.cmd', 'Install.ps1', 'Uninstall.cmd', 'companion/naab-companion.exe'] : ['Install.command', 'companion/naab-companion']) assert.ok(declared.has(name), `Missing ${name}`);
if (!windows && process.platform !== 'win32') {
  for (const name of ['Install.command', 'companion/naab-companion']) assert.ok((await lstat(path.join(directory, name))).mode & 0o111, `${name} lost executable permission`);
}
console.log(`PASS: ${actual.length} browser-package files verified against BUILD-INFO; source ${info.sourceCommit}; no DNS helpers or development runtimes.`);
