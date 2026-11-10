#!/usr/bin/env node
// Rust-owned Copilot turn via the SDK reference implementation.
// Invoked by copilot_bridge.rs: `node copilot_runtime.mjs <runtime> <token> <model> <prompt>`
// Reads prompt from stdin, writes JSON result to stdout.
// The protocol ( CopilotClient, createSession, sendAndWait ) is the SDK's own — this wrapper
// keeps the Rust side honest without reimplementing vscode-jsonrpc framing.
import fs from 'node:fs';
import { createRequire } from 'module';
import path from 'node:path';

const [runtimePath, token, model, prompt] = process.argv.slice(2);
if (!runtimePath || !token || !prompt) {
  console.error('usage: copilot_runtime.mjs <runtimePath> <token> <model> <prompt>');
  process.exit(2);
}
if (!fs.existsSync(runtimePath)) {
  console.error(`runtime not found: ${runtimePath}`);
  process.exit(2);
}
const require = createRequire(import.meta.url);
let client;
try {
  // Resolve SDK via the packaged path; in dev it's under node_modules/.pnpm
  // Find SDK root by searching upward for @github/copilot-sdk
  let cur = path.resolve('.');
  let sdkRoot = null;
  for (let i = 0; i < 8; i++) {
    const candidate = path.join(cur, 'node_modules', '@github', 'copilot-sdk');
    if (fs.existsSync(candidate)) { sdkRoot = candidate; break; }
    const pnpmCandidate = path.join(cur, 'node_modules', '.pnpm');
    if (fs.existsSync(pnpmCandidate)) {
      const entries = fs.readdirSync(pnpmCandidate).filter(e => e.startsWith('@github+copilot-sdk@'));
      if (entries.length) {
        sdkRoot = path.join(pnpmCandidate, entries[0], 'node_modules', '@github', 'copilot-sdk');
        if (fs.existsSync(sdkRoot)) break;
      }
    }
    cur = path.dirname(cur);
  }
  const entry = sdkRoot ? path.join(sdkRoot, 'dist/cjs/index.js') : null;
  if (!entry || !fs.existsSync(entry)) {
    console.error('copilot-sdk not found under node_modules');
    process.exit(2);
  }
  const mod = await import(entry);
  const { CopilotClient } = mod;
  client = new CopilotClient({
    connection: { kind: 'stdio', path: runtimePath },
    gitHubToken: token,
    workingDirectory: process.cwd(),
  });
  await client.start();
  const session = await client.createSession({ model: model || 'gpt-4o-mini' });
  const result = await session.sendAndWait({ prompt });
  // result is the assistant.message envelope
  const content = result?.data?.content ?? result?.content ?? '';
  if (!content || typeof content !== 'string') {
    console.error('empty assistant.message content');
    console.log(JSON.stringify({ error: 'empty_content', result }));
    process.exit(1);
  }
  console.log(JSON.stringify({ content, model: result?.data?.model ?? model, session_id: session.sessionId }));
  try { await session.disconnect(); } catch {}
  try { await client.stop(); } catch {}
} catch (e) {
  console.error(e?.message ?? String(e));
  console.log(JSON.stringify({ error: String(e?.message ?? e) }));
  try { if (client) await client.stop(); } catch {}
  process.exit(1);
}
