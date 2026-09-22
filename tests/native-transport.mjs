import { spawn } from 'node:child_process';
import { endianness } from 'node:os';

const littleEndian = endianness() === 'LE';
export function frame(message) {
  const json = Buffer.from(JSON.stringify(message));
  const header = Buffer.alloc(4);
  header[littleEndian ? 'writeUInt32LE' : 'writeUInt32BE'](json.length);
  return Buffer.concat([header, json]);
}

export function exchange(binary, input, { fragment = false, timeoutMs = 10_000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(binary, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
    const stdout = []; const stderr = [];
    let total = 0;
    const timer = setTimeout(() => { child.kill(); reject(new Error('Native host integration timeout')); }, timeoutMs);
    child.on('error', error => { clearTimeout(timer); reject(error); });
    child.stdout.on('data', chunk => {
      total += chunk.length;
      if (total > 4 * 1024 * 1024) { child.kill(); reject(new Error('Unexpectedly large native output')); }
      else stdout.push(chunk);
    });
    child.stderr.on('data', chunk => stderr.push(chunk));
    child.stdin.on('error', () => {}); // Early rejection may close stdin before the test finishes writing.
    child.on('close', code => {
      clearTimeout(timer);
      try {
        const bytes = Buffer.concat(stdout); const messages = []; let offset = 0;
        while (offset < bytes.length) {
          if (bytes.length - offset < 4) throw new Error('Truncated response header');
          const size = bytes[littleEndian ? 'readUInt32LE' : 'readUInt32BE'](offset); offset += 4;
          if (size > 1024 * 1024 || size > bytes.length - offset) throw new Error('Invalid response size');
          messages.push(JSON.parse(bytes.subarray(offset, offset + size).toString('utf8'))); offset += size;
        }
        resolve({ code, messages, stderr: Buffer.concat(stderr).toString('utf8') });
      } catch (error) { reject(error); }
    });
    if (!fragment) child.stdin.end(input);
    else {
      // Separate header writes exercise partial reads without introducing long sleeps.
      child.stdin.write(input.subarray(0, 1));
      setImmediate(() => { child.stdin.write(input.subarray(1, 3)); setImmediate(() => child.stdin.end(input.subarray(3))); });
    }
  });
}

export function nativeTransport(binary, options = {}) {
  return async (host, request) => {
    if (host !== 'com.naab.companion') throw new Error('Unexpected host name');
    const result = await exchange(binary, frame(request), { fragment: true, ...options });
    if (result.code !== 0 || result.messages.length !== 1) throw new Error(`Native transport failed: ${result.stderr}`);
    return result.messages[0];
  };
}
