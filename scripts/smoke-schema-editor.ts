/** Exercise the installed Taplo language server against the committed schema. */
import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

type Message = {
  id?: number;
  method?: string;
  params?: { uri?: string; diagnostics?: { message: string }[]; items?: unknown[] };
  result?: unknown;
  error?: unknown;
};
const version = Bun.spawnSync(["taplo", "--version"]);
if (version.exitCode !== 0) throw new Error("Install Taplo to run this optional editor smoke.");
console.log(version.stdout.toString().trim());
const directory = await mkdtemp(join(tmpdir(), "huterm-schema-editor-"));
const server = spawn("taplo", ["lsp", "stdio"], { stdio: "pipe", cwd: directory });
const events = new EventEmitter();
let output = Buffer.alloc(0);
let stderr = "";
let sequence = 0;
let lastNotification = "";
server.stderr.on("data", (chunk: Buffer) => { stderr = (stderr + chunk.toString()).slice(-16000); });
server.on("error", error => events.emit("failure", error));
server.on("exit", code => events.emit("failure", new Error(`Taplo exited ${code}: ${stderr}`)));
function send(message: object): void {
  const payload = Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...message }));
  server.stdin.write(`Content-Length: ${payload.length}\r\n\r\n`);
  server.stdin.write(payload);
}
server.stdout.on("data", (chunk: Buffer) => {
  output = Buffer.concat([output, chunk]);
  for (;;) {
    const boundary = output.indexOf("\r\n\r\n");
    if (boundary < 0) return;
    const length = Number(/content-length:\s*(\d+)/i.exec(output.subarray(0, boundary).toString())?.[1]);
    if (!Number.isFinite(length)) { events.emit("failure", new Error("Invalid LSP frame")); return; }
    if (output.length < boundary + 4 + length) return;
    const message = JSON.parse(output.subarray(boundary + 4, boundary + 4 + length).toString()) as Message;
    output = output.subarray(boundary + 4 + length);
    if (message.method === "workspace/configuration") send({ id: message.id, result: message.params?.items?.map(() => ({ schema: { catalogs: [] } })) ?? [] });
    else { if (message.method) lastNotification = JSON.stringify(message); events.emit("message", message); }
  }
});
function waitFor(predicate: (message: Message) => boolean, label: string): Promise<Message> {
  return new Promise((resolve, reject) => {
    const cleanup = () => { clearTimeout(timer); events.off("message", onMessage); events.off("failure", onFailure); };
    const onMessage = (message: Message) => { if (predicate(message)) { cleanup(); resolve(message); } };
    const onFailure = (error: Error) => { cleanup(); reject(error); };
    const timer = setTimeout(() => onFailure(new Error(`Timed out waiting for ${label}: ${lastNotification}\n${stderr}`)), 15000);
    events.on("message", onMessage);
    events.on("failure", onFailure);
  });
}
async function request(method: string, params: unknown): Promise<unknown> {
  const id = ++sequence;
  const response = waitFor(message => message.id === id, method);
  send({ id, method, params });
  const message = await response;
  if (message.error) throw new Error(JSON.stringify(message.error));
  return message.result;
}
async function completions(uri: string, line: number, character: number): Promise<string[]> {
  const result = await request("textDocument/completion", { textDocument: { uri }, position: { line, character } }) as { label: string }[] | { items: { label: string }[] } | null;
  return (Array.isArray(result) ? result : result?.items ?? []).map(item => item.label.replaceAll('"', ""));
}
function requireLabels(actual: string[], expected: string[]): void {
  for (const name of expected) if (!actual.includes(name)) throw new Error(`Missing ${name} completion: ${JSON.stringify(actual)}`);
}
const schema = pathToFileURL(resolve(import.meta.dir, "../schemas/huterm.schema.json")).href;
const uri = pathToFileURL(join(directory, "config.toml")).href;
try {
  await request("initialize", { processId: process.pid, rootUri: pathToFileURL(directory).href, workspaceFolders: [{ uri: pathToFileURL(directory).href, name: "schema smoke" }], capabilities: { workspace: { configuration: true } } });
  send({ method: "initialized", params: {} });
  let version = 0;
  for (const [command, argument] of [["select_tab", "index"], ["rename_tab", "name"], ["select_tab", "index"]]) {
    version++;
    const text = `#:schema ${schema}\n[[keybinding]]\nkey = "ctrl-1"\ncommand = "${command}"\nargs = {  }\n`;
    await writeFile(join(directory, "config.toml"), text);
    const diagnostic = waitFor(message => message.method === "textDocument/publishDiagnostics" && message.params?.uri === uri && (message.params.diagnostics?.some(item => item.message.includes(`"${argument}" is a required property`)) ?? false), `${command} argument diagnostic`);
    if (version === 1) send({ method: "textDocument/didOpen", params: { textDocument: { uri, languageId: "toml", version, text } } });
    else send({ method: "textDocument/didChange", params: { textDocument: { uri, version }, contentChanges: [{ text }] } });
    await diagnostic;
    const labels = await completions(uri, 3, 13);
    requireLabels(labels, ["select_tab", "rename_tab", "copy", "unbind"]);
    console.log(`${command}: required ${argument} diagnostic; ${labels.length} command completions`);
    console.log(`${command}: argument completions ${JSON.stringify(await completions(uri, 4, 9))}`);
  }
  const engineUri = pathToFileURL(join(directory, "engine.toml")).href;
  const diagnostic = waitFor(message => message.method === "textDocument/publishDiagnostics" && message.params?.uri === engineUri && (message.params.diagnostics?.length ?? 0) > 0, "engine enum diagnostic");
  send({ method: "textDocument/didOpen", params: { textDocument: { uri: engineUri, languageId: "toml", version: 1, text: `#:schema ${schema}\n[terminal]\nengine = "unknown"\n` } } });
  await diagnostic;
  requireLabels(await completions(engineUri, 2, 12), ["alacritty", "ghostty"]);
  console.log("engine: alacritty and ghostty complete; unknown engine diagnosed");
  await request("shutdown", null);
  send({ method: "exit" });
} finally {
  server.kill();
  await rm(directory, { recursive: true, force: true });
}
