/** Fold edits from the build tree into one patch, then replay its successors. */
import { dlopen } from "bun:ffi";
import { closeSync, cpSync, existsSync, mkdirSync, mkdtempSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { applyPatch, checkIdentity, differences, extract, git, gitBytes, id, reproduce, treeEntries, type Source } from "./vendor";

type State = {
  schema: 1;
  source: Source;
  target: number;
  patches: string[];
  original: string[];
  phase: "editing" | "replaying" | "publishing";
  next: number;
  head: string;
  delta?: string;
  heads: string[];
  desired?: [string, string][];
  candidates?: string[];
};
const location = (storage: string, source: Source) => join(storage, "sessions", source.name);
const work = (session: string) => join(session, "repo/tree");
const sg = (session: string, args: string[]) => git(join(session, "repo"), args).trimEnd();
const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
function atomic(file: string, text: string) {
  const temporary = `${file}.${process.pid}.tmp`;
  try { writeFileSync(temporary, text); renameSync(temporary, file); }
  finally { rmSync(temporary, { force: true }); }
}
function retire(session: string) {
  // Remove the active name atomically before recursive cleanup can be interrupted.
  const finished = `${session}.finished-${Date.now()}-${process.pid}`;
  renameSync(session, finished);
  rmSync(finished, { recursive: true, force: true });
}
const save = (session: string, state: State) => atomic(join(session, "state.json"), `${JSON.stringify(state, null, 2)}\n`);
function read(session: string): State {
  if (!existsSync(join(session, "state.json"))) throw new Error(`no active session: ${session}`);
  const state = JSON.parse(readFileSync(join(session, "state.json"), "utf8")) as State;
  if (state.schema !== 1 || !["editing", "replaying", "publishing"].includes(state.phase)) throw new Error(`invalid session: ${session}`);
  return state;
}

/** Kernel locks survive neither errors nor process death; sessions survive both. */
export async function withVendorLock<T>(storage: string, name: string, action: () => Promise<T>): Promise<T> {
  mkdirSync(storage, { recursive: true });
  const fd = openSync(join(storage, `${name}.lock`), "a");
  try {
    const libc = dlopen(process.platform === "darwin" ? "/usr/lib/libSystem.B.dylib" : "libc.so.6", { flock: { args: ["i32", "i32"], returns: "i32" } });
    try {
      if (libc.symbols.flock(fd, 6) !== 0) throw new Error(`another vendor command is running for ${name}; retry after it finishes`);
      return await action();
    } finally { libc.close(); }
  } finally { closeSync(fd); }
}

export function sessionStatus(source: Source, storage: string): string {
  const session = location(storage, source);
  if (!existsSync(session)) return `${source.name}: no active session`;
  const state = read(session);
  const target = state.source.patches[state.target]!.name;
  const action = state.phase === "editing" ? "finish" : "continue";
  return `${source.name}: ${state.phase}, target ${target}\nWorkspace: ${join(session, "repo")}\nNext: mise run vendor:${action} -- ${source.name}`;
}

function unchangedRecipe(state: State, source: Source, root: string) {
  if (JSON.stringify(state.source) !== JSON.stringify(source)) throw new Error("vendor manifest changed during the session; restore it or cancel and restart");
  for (const [index, patch] of source.patches.entries()) {
    const current = readFileSync(join(root, patch.file), "utf8");
    if (current !== state.patches[index] && !(state.phase === "publishing" && current === state.candidates?.[index])) {
      throw new Error(`patch changed during the session: ${patch.file}; preserve those edits before restoring the recipe`);
    }
  }
}

function replaceWork(session: string, source: string) {
  rmSync(work(session), { recursive: true, force: true });
  cpSync(source, work(session), { recursive: true, dereference: false, verbatimSymlinks: true });
}
function commit(session: string, message: string): string {
  sg(session, ["add", "--all", "--force", "--", "tree"]);
  sg(session, ["commit", "--quiet", "--allow-empty", "-m", message]);
  return sg(session, ["rev-parse", "HEAD"]);
}

async function start(source: Source, root: string, storage: string, target: string, archive: string, adopt: boolean) {
  const destination = location(storage, source);
  if (existsSync(destination)) throw new Error(`${sessionStatus(source, storage)}\nResume or cancel the existing session first.`);
  const index = source.patches.findIndex((patch) => patch.name === target);
  if (index < 0) throw new Error(`unknown patch ${target}; available: ${source.patches.map((patch) => patch.name).join(", ")}`);
  checkIdentity(source, join(root, id(source)));
  treeEntries(join(root, id(source)));
  if (!adopt) await reproduce(source, root, archive);
  mkdirSync(join(storage, "sessions"), { recursive: true });
  const stage = mkdtempSync(join(storage, "sessions", ".start-"));
  try {
    mkdirSync(join(stage, "repo"));
    sg(stage, ["init", "--quiet", "--template="]);
    // Do not inherit a user's global ignores, attributes, signing, or hooks.
    sg(stage, ["config", "gc.auto", "0"]);
    sg(stage, ["config", "maintenance.auto", "false"]);
    sg(stage, ["config", "core.excludesFile", "/dev/null"]);
    sg(stage, ["config", "core.attributesFile", "/dev/null"]);
    mkdirSync(join(stage, "repo/.git/info"), { recursive: true });
    writeFileSync(join(stage, "repo/.git/info/attributes"), "* -text -eol -ident -filter -working-tree-encoding !diff !merge -export-ignore -export-subst\n");
    await extract(source, archive, work(stage));
    const heads = [commit(stage, "Published archive")];
    const patches = source.patches.map((patch) => readFileSync(join(root, patch.file), "utf8"));
    for (const [i, patch] of source.patches.entries()) {
      const file = join(stage, "apply.patch");
      writeFileSync(file, patches[i]!);
      if (patches[i]!.trim()) sg(stage, ["apply", "--whitespace=nowarn", "--directory=tree", file]);
      heads.push(commit(stage, patch.name));
    }
    if (!adopt && differences(treeEntries(work(stage)), treeEntries(join(root, id(source)))).length) throw new Error("vendor source changed while starting the session; retry");
    const state: State = { schema: 1, source, target: index, patches, original: heads, phase: "editing", next: 0, head: heads.at(-1)!, heads: [] };
    save(stage, state);
    renameSync(stage, destination);
  } finally { rmSync(stage, { recursive: true, force: true }); }
}

function conflict(session: string, state: State): never {
  const patch = state.next === 0 ? state.source.patches[state.target]!.name : state.source.patches[Math.min(state.target + state.next, state.source.patches.length - 1)]!.name;
  throw new Error(`Resolve ${patch} in ${join(session, "repo/tree")} and stage the resolved files:\n  cd ${shellQuote(join(session, "repo"))}\n  git add --all --force -- tree\nThen, from Huterm: mise run vendor:continue -- ${state.source.name}\nKeep the intended final source in the vendored directory. Do not run rebase, reset, or cherry-pick manually.`);
}

function verifyDesired(session: string, state: State, root: string) {
  const desired = new Map(state.desired!);
  const sourceChanged = differences(desired, treeEntries(join(root, id(state.source))));
  if (sourceChanged.length) throw new Error(`vendored source changed after finish began: ${sourceChanged.join(", ")}; run vendor:reopen to include further edits, then finish again`);
  const mismatch = differences(desired, treeEntries(work(session)));
  if (mismatch.length) throw new Error(`replayed stack differs from the tested vendor tree: ${mismatch.join(", ")}\nCorrect the current patch in ${work(session)}, then run vendor:continue. The vendored source is unchanged.`);
}

function patchBetween(session: string, before: string, after: string): string {
  return git(join(session, "repo"), ["diff", "--relative=tree", "--src-prefix=a/", "--dst-prefix=b/", "--no-ext-diff", "--no-textconv", "--binary", "--full-index", before, after, "--", "tree"]);
}

async function drive(session: string, state: State, source: Source, root: string) {
  unchangedRecipe(state, source, root);
  if (state.phase === "replaying") {
    // Recording HEAD after each step makes a crash after Git committed resumable.
    const steps = [state.delta!, ...state.original.slice(state.target + 2)];
    while (state.next < steps.length) {
      const current = sg(session, ["rev-parse", "HEAD"]);
      if (current === state.head) {
        if (existsSync(join(session, "repo/.git/CHERRY_PICK_HEAD"))) {
          if (sg(session, ["diff", "--name-only", "--diff-filter=U"])) conflict(session, state);
          sg(session, ["diff", "--exit-code"]);
          sg(session, ["commit", "--quiet", "--allow-empty", "-m", `Resolve ${source.patches[state.target + state.next]!.name}`]);
        } else {
          if (sg(session, ["status", "--porcelain"])) throw new Error("unexpected edits in the replay workspace; preserve them before continuing");
          try { sg(session, ["cherry-pick", "--allow-empty", steps[state.next]!]); }
          catch (error) {
            if (existsSync(join(session, "repo/.git/CHERRY_PICK_HEAD"))) conflict(session, state);
            throw error;
          }
        }
      }
      const head = sg(session, ["rev-parse", "HEAD"]);
      if (head === state.head || sg(session, ["show", "-s", "--format=%P", head]) !== state.head) throw new Error("unexpected replay history; preserve the session and inspect its state.json");
      state.head = head;
      state.heads.push(head);
      state.next++;
      save(session, state);
    }
    const actualHead = sg(session, ["rev-parse", "HEAD"]);
    if (actualHead !== state.head) {
      if (sg(session, ["show", "-s", "--format=%P", actualHead]) !== state.head) throw new Error("unexpected replay history");
      state.head = actualHead;
      state.heads[state.heads.length - 1] = actualHead;
      save(session, state);
    }
    // A final-tree mismatch may be repaired on the last patch without rerunning replay.
    if (sg(session, ["status", "--porcelain"])) {
      if (sg(session, ["diff", "--name-only", "--diff-filter=U"])) conflict(session, state);
      if (differences(new Map(state.desired!), treeEntries(work(session))).length) {
        verifyDesired(session, state, root);
      }
      state.head = commit(session, "Correct final replay result");
      state.heads[state.heads.length - 1] = state.head;
      save(session, state);
    }
    verifyDesired(session, state, root);
    state.candidates = state.patches.map((patch, index) => {
      if (index < state.target) return patch;
      const updated = patchBetween(session, state.heads[index]!, state.heads[index + 1]!);
      const original = patchBetween(session, state.original[index]!, state.original[index + 1]!);
      return updated === original ? patch : updated;
    });
    // Check the generated patch text itself, not just the private Git history.
    const verify = join(session, "verify");
    rmSync(verify, { recursive: true, force: true });
    mkdirSync(verify);
    const archive = git(join(session, "repo"), ["rev-parse", `${state.original[0]}:tree`]).trim();
    // Export binary-safe through git archive rather than stdout's text helper.
    await new Bun.Archive(gitBytes(join(session, "repo"), ["archive", archive])).extract(verify);
    for (const [index, candidate] of state.candidates.entries()) {
      const file = join(session, `candidate-${index}.patch`);
      writeFileSync(file, candidate);
      applyPatch(verify, file);
    }
    if (differences(new Map(state.desired!), treeEntries(verify)).length) throw new Error("generated patches do not reproduce the tested source");
    state.phase = "publishing";
    save(session, state);
  }
  verifyDesired(session, state, root);
  unchangedRecipe(state, source, root);
  // Publication can resume after interruption; original patches stay in state.json.
  for (const [index, patch] of source.patches.entries()) atomic(join(root, patch.file), state.candidates![index]!);
  retire(session);
}

export async function runSession(mode: string, source: Source, root: string, storage: string, target?: string, archive?: string, adopt = false): Promise<void> {
  if (mode === "start") return start(source, root, storage, target!, archive!, adopt);
  const session = location(storage, source);
  const state = read(session);
  if (mode === "cancel") {
    if (state.phase === "publishing") {
      unchangedRecipe(state, source, root);
      for (const [index, patch] of source.patches.entries()) atomic(join(root, patch.file), state.patches[index]!);
    }
    // Retain the baseline and conflict work for recovery; never revert build inputs.
    const backup = `${session}.cancelled-${Date.now()}`;
    renameSync(session, backup);
    console.log(`Cancelled; source edits preserved. Session backup: ${backup}`);
    return;
  }
  unchangedRecipe(state, source, root);
  if (mode === "reopen") {
    // Copy first so a failed backup never removes the only active session.
    const backup = `${session}.reopened-${Date.now()}`;
    cpSync(session, backup, { recursive: true, dereference: false, verbatimSymlinks: true });
    if (state.phase === "publishing") {
      for (const [index, patch] of source.patches.entries()) atomic(join(root, patch.file), state.patches[index]!);
    }
    state.phase = "editing";
    state.next = 0;
    state.heads = [];
    delete state.delta;
    delete state.desired;
    delete state.candidates;
    save(session, state);
    console.log(`Editing reopened; source edits preserved. Replay backup: ${backup}`);
    return;
  }
  if (mode === "finish") {
    if (state.phase !== "editing") throw new Error(`finish already started; use vendor:continue -- ${source.name}`);
    const current = join(root, id(source));
    checkIdentity(source, current);
    const desired = treeEntries(current);
    sg(session, ["reset", "--hard", state.original.at(-1)!]);
    sg(session, ["clean", "-fdx"]);
    if (!differences(treeEntries(work(session)), desired).length) {
      retire(session);
      return;
    }
    replaceWork(session, current);
    state.delta = commit(session, `Edits for ${source.patches[state.target]!.name}`);
    state.desired = [...desired];
    state.head = state.original[state.target + 1]!;
    state.heads = state.original.slice(0, state.target + 1);
    sg(session, ["reset", "--hard", state.head]);
    state.phase = "replaying";
    save(session, state);
  } else if (state.phase === "editing") throw new Error(`edit the vendor source, then use vendor:finish -- ${source.name}`);
  await drive(session, state, source, root);
}
