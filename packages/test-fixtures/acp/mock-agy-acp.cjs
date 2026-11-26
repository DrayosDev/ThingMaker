// Native ACP contract double, not a real provider. Its MCP client launches the
// actual ThingMaker relay, so tests exercise session-local authentication/tools.
const { spawn } = require('node:child_process');
const { createInterface } = require('node:readline');
const fs = require('node:fs');
const args = process.argv.slice(2);
const capture = args.find(x => x.startsWith('--capture='))?.slice(10);
const log = value => { if (capture) fs.appendFileSync(capture, JSON.stringify(value) + '\n'); };
const send = value => process.stdout.write(JSON.stringify({ jsonrpc: '2.0', ...value }) + '\n');
// IDs captured from native agy 1.5.14 (distinct from stream-json bridge IDs).
const options = { model: 'default', thought_level: 'low', mode: 'plan' };
const configOptions = () => Object.entries(options).map(([id, currentValue]) => ({ id, name: id, category: id, type: 'select', currentValue, options: [currentValue, 'gemini-test', 'high', 'accept-edits'].map(value => ({ value, name: value })) }));
let servers = [], session = 'native-conversation', callback = 1000;
const replies = new Map(), children = new Set();
function request(method, params) {
  const id = ++callback;
  return new Promise((resolve, reject) => { replies.set(id, { resolve, reject }); send({ id, method, params }); });
}
function update(value) { send({ method: 'session/update', params: { sessionId: session, update: value } }); }
async function mcpRun() {
  const server = servers.find(x => x.name === 'team');
  if (!server) throw Error('team server missing');
  const env = { ...process.env, ...Object.fromEntries(server.env.map(x => [x.name, x.value])) };
  const child = spawn(server.command, server.args, { env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  children.add(child);
  child.stderr.resume();
  let sequence = 0;
  const pending = new Map();
  createInterface({ input: child.stdout }).on('line', line => {
    const result = JSON.parse(line), receiver = pending.get(result.id);
    if (receiver) { pending.delete(result.id); result.error ? receiver.reject(Error(result.error.message)) : receiver.resolve(result.result); }
  });
  child.on('error', error => { for (const p of pending.values()) p.reject(error); });
  child.on('exit', () => { children.delete(child); for (const p of pending.values()) p.reject(Error('relay exited')); });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence; pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
  try {
    await call('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'mock-agy', version: '1' } });
    const tools = await call('tools/list');
    if (!tools.tools.some(x => x.name === 'delegate')) throw Error('delegate missing');
    const workers = await call('tools/call', { name: 'list_workers', arguments: {} });
    log({ workers });
    if (!args.includes('--delegate')) return;
    const delegated = await call('tools/call', { name: 'delegate', arguments: { task: 'Reply with worker-result', worker: 'helper' } });
    if (delegated.isError) throw Error(JSON.stringify(delegated));
    const job = JSON.parse(delegated.content[0].text);
    const result = await call('tools/call', { name: 'await_jobs', arguments: { job_ids: [job.job_id], timeout_seconds: 10 } });
    log({ delegationResult: result });
    if (!JSON.stringify(result).includes('worker-result')) throw Error('worker result missing');
  } finally { child.stdin.end(); }
}
async function handle(message) {
  if (!message.method) {
    const pending = replies.get(message.id);
    if (pending) { replies.delete(message.id); message.error ? pending.reject(Error(message.error.message)) : pending.resolve(message.result); }
    return;
  }
  log(message);
  const { id, method, params = {} } = message;
  if (id === undefined) return;
  let result;
  switch (method) {
    case 'initialize':
      if (!args.includes('acp') || args.includes('-p=')) throw Error('not native ACP');
      if (args.includes('--fail-init')) { send({ id, error: { code: -32601, message: 'ACP unavailable' } }); return; }
      result = { protocolVersion: 1, agentInfo: { name: 'antigravity-cli', version: 'mock' }, agentCapabilities: { loadSession: true }, authMethods: [] }; break;
    case 'session/new': case 'session/load':
      if (args.includes('--fail-load') && method === 'session/load') { send({ id, error: { code: -32602, message: 'Resource not found' } }); return; }
      servers = params.mcpServers;
      session = args.includes('--wrong-load') ? 'unexpected-new-conversation' : (params.sessionId || session);
      result = { sessionId: session, configOptions: configOptions() }; break;
    case 'session/set_config_option':
      if (!(params.configId in options)) throw Error(`unknown config ID ${params.configId}`);
      options[params.configId] = params.value;
      result = { configOptions: configOptions() }; break;
    case 'session/prompt': {
      const permission = await request('session/request_permission', { sessionId: session, toolCall: { toolCallId: 'mock-tool', title: 'Team MCP', kind: 'other' }, options: [{ optionId: 'yes', name: 'Allow', kind: 'allow_once' }] });
      log({ permission });
      if (permission.outcome.outcome !== 'selected') { result = { stopReason: 'cancelled' }; break; }
      if (servers.length) await mcpRun();
      update({ sessionUpdate: 'tool_call', toolCallId: 'mock-tool', title: 'Team MCP', kind: 'other', status: 'completed' });
      update({ sessionUpdate: 'agent_message_chunk', content: { type: 'text', text: 'worker-result' } });
      result = { stopReason: 'end_turn' }; break;
    }
    default: send({ id, error: { code: -32601, message: `unknown method ${method}` } }); return;
  }
  send({ id, result });
}
const input = createInterface({ input: process.stdin });
input.on('line', line => {
  const value = JSON.parse(line);
  handle(value).catch(error => send({ id: value.id, error: { code: -32603, message: error.message } }));
});
input.on('close', () => { for (const child of children) child.kill(); process.exit(); });
