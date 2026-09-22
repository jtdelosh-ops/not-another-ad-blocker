import { build } from 'esbuild';
import { cp, mkdir } from 'node:fs/promises';

await mkdir('dist', { recursive: true });
await build({ entryPoints: { background: 'src/background/index.ts', content: 'src/content/index.ts', popup: 'src/popup/index.ts', options: 'src/options/index.ts', activity: 'src/activity/index.ts' }, outdir: 'dist', bundle: true, target: 'chrome121', format: 'iife', sourcemap: true });
await cp('public', 'dist', { recursive: true });
await cp('manifest.json', 'dist/manifest.json');
// A browser-independent client for the real native-wire integration harness.
await build({ entryPoints: ['src/shared/native-client.ts'], outfile: 'lib/native-client.mjs', bundle: true, platform: 'node', format: 'esm', target: 'node22' });
console.log('Built extension/dist (load this directory as an unpacked extension).');
