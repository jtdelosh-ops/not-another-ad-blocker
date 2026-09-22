// A loopback-only visual check. No downloads, ad networks, or rule changes.
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

const html = await readFile(new URL('../tests/fixtures/local-check.html', import.meta.url), 'utf8');
const banner = `<svg xmlns="http://www.w3.org/2000/svg" width="640" height="210" viewBox="0 0 640 210"><rect width="640" height="210" rx="16" fill="#ffc692"/><circle cx="550" cy="45" r="100" fill="#ee9275"/><circle cx="570" cy="175" r="65" fill="#d36857"/><text x="32" y="85" fill="#302520" font-family="Arial,sans-serif" font-size="17" font-weight="bold">HARMLESS LOCAL TEST</text><text x="32" y="133" fill="#302520" font-family="Arial,sans-serif" font-size="36" font-weight="bold">This banner loaded.</text></svg>`;

export async function startTestPage({ port = 0 } = {}) {
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Port must be an integer from 0 to 65535.');
  const server = createServer((request, response) => {
    response.setHeader('Cache-Control', 'no-store');
    response.setHeader('X-Content-Type-Options', 'nosniff');
    response.setHeader('Referrer-Policy', 'no-referrer');
    if (!['GET', 'HEAD'].includes(request.method)) { response.writeHead(405); response.end(); return; }
    const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
    let body;
    if (pathname === '/') {
      response.setHeader('Content-Type', 'text/html; charset=utf-8'); body = html;
    } else if (pathname === '/reference.svg' || pathname === '/adimage.svg') {
      // Identical bytes; only the request path differs. EasyList's /adimage.
      // image filter applies to the latter, without a new local filter import.
      response.setHeader('Content-Type', 'image/svg+xml'); body = banner;
    } else { response.writeHead(404); response.end(); return; }
    response.end(request.method === 'HEAD' ? undefined : body);
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => { server.off('error', reject); resolve(); });
  });
  return {
    server,
    url: `http://127.0.0.1:${server.address().port}/`,
    close: () => new Promise((resolve, reject) => {
      server.close(error => error ? reject(error) : resolve());
      server.closeIdleConnections();
    }),
  };
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  const args = process.argv.slice(2);
  if (args.length && (args.length !== 2 || args[0] !== '--port' || !/^\d+$/.test(args[1]))) {
    throw new Error('Usage: node scripts/test-page.mjs [--port 8765]');
  }
  const check = await startTestPage({ port: args.length ? Number(args[1]) : 8765 });
  console.log(`Local NAAB check: ${check.url}`);
  console.log('Copy this address into the Chrome profile where NAAB is installed. Keep other blockers off.');
  console.log('NAAB OFF: both banners load. NAAB ON with EasyList: only the normal banner loads.');
  console.log('This server listens only on this computer. Press Ctrl+C to stop it.');
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { void check.close().then(() => process.exit(0)); });
}
