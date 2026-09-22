import { build } from 'esbuild';
import { readdir, mkdir } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
const entries = (await readdir('tests')).filter(name => name.endsWith('.test.ts')).map(name => `tests/${name}`);
await mkdir('.test-build', { recursive: true });
await build({ entryPoints: entries, outdir: '.test-build', outExtension: { '.js': '.mjs' }, bundle: true, platform: 'node', format: 'esm', target: 'node22' });
const result = spawnSync(process.execPath, ['--test', ...entries.map(name => `.test-build/${name.split('/').at(-1).replace(/\.ts$/, '.mjs')}`)], { stdio: 'inherit' });
process.exit(result.status ?? 1);
