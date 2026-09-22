// Opt-in integration check: contacts only the fixed official list sources.
// Set NAAB_DATA_DIR to an isolated cache and pass an output JSON path for QA.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { NativeClient } from '../extension/lib/native-client.mjs';
import { nativeTransport } from './native-transport.mjs';
if (!process.env.NAAB_DATA_DIR || !process.argv[2]) throw new Error('Set NAAB_DATA_DIR to a test cache and pass an output JSON path.');
const binary = process.env.NAAB_BINARY || fileURLToPath(new URL(`../companion/target/debug/naab-companion${process.platform === 'win32' ? '.exe' : ''}`, import.meta.url));
const client = new NativeClient(nativeTransport(binary, {timeoutMs:120_000}));
let phase;
const start = Date.now();
const state = await client.refresh(['easylist','easyprivacy'],27800, progress => {
  if (progress.phase !== phase) { phase=progress.phase; console.log(`Subscription check: ${phase}`); }
});
assert.equal(state.manifest.lists.length,2);
assert.ok(state.compiled.networkRules.some(r=>r.action.type==='block'), 'Real subscriptions must produce actual blocking rules, not only allowances');
assert.ok(state.compiled.cosmeticRules.length>0, 'Real subscriptions should retain supported site-scoped cosmetics');
assert.equal(new Set(state.compiled.networkRules.map(r=>r.id)).size,state.compiled.networkRules.length);
assert.ok(state.compiled.networkRules.length<=27800);
assert.ok(state.compiled.diagnostics.length<=200);
const domains=state.compiled.networkRules.flatMap(r=>r.condition.requestDomains||[]).length;
assert.ok(domains>1000, 'Verify real domain compaction, not a tiny example');
await writeFile(process.argv[2],JSON.stringify(state));
console.log(JSON.stringify({elapsedSeconds:(Date.now()-start)/1000,stats:state.manifest.stats,coverage:state.manifest.coverage,packedDomainEntries:domains,lists:state.manifest.lists},null,2));
console.log('PASS: real HTTPS downloads, Rust compilation/cache, native paging across processes, and actual extension client validation.');
