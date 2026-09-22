/** Rebuild and relaunch loop shared by the macOS and Linux VM dev sessions. */

export type DevActions = {
  /** Build on the host. A non-zero status keeps the running app alive. */
  rebuild: () => Promise<number>;
  /** Copy the fresh build into the guest. */
  stage: () => Promise<number>;
  /** Start the app in the guest; resolves with its exit status. */
  launch: () => Promise<number>;
  /** Ask the running guest app to exit, and settle its host-side process. */
  stopApp: () => Promise<void>;
  /** Stop an in-flight build so quitting does not wait for it. */
  cancelBuild?: () => void;
  log: (message: string) => void;
};

export const KEY_HELP = "r = rebuild and relaunch, w = toggle watch, q = quit";

/**
 * Keeps one guest app instance under host control while the VM stays up, so an
 * edit costs a rebuild rather than another boot.
 */
export class DevSession {
  private running: Promise<number> | undefined;
  private cycling: Promise<number> | undefined;
  private busy = false;
  private quit = false;
  private pending = false;
  private status = 0;
  private finish!: () => void;
  private finished = new Promise<void>((resolve) => { this.finish = resolve; });

  /** Without a terminal there is nobody to press keys, so the first instance
   *  exiting ends the session, matching a plain run. */
  constructor(private actions: DevActions, private watching = false, private autoQuit = false) {}

  get isWatching(): boolean { return this.watching; }
  get isBusy(): boolean { return this.busy; }
  get isRunning(): boolean { return this.running !== undefined; }

  /** Builds, stages, and starts the first instance. */
  async start(): Promise<number> {
    const status = await this.cycle(true);
    if (status === 0) this.actions.log(`watch ${this.watching ? "on" : "off"}; ${KEY_HELP}`);
    return status;
  }

  /** Resolves once the session quits, after the app has stopped. */
  async wait(): Promise<number> {
    await this.finished;
    return this.status;
  }

  async key(key: string): Promise<void> {
    switch (key.toLowerCase()) {
      case "r": await this.restart(); break;
      case "w":
        this.watching = !this.watching;
        this.actions.log(`watch ${this.watching ? "on" : "off"}`);
        break;
      case "q": await this.stop(); break;
      default: break;
    }
  }

  /** A debounced source change; ignored unless watching. */
  async changed(): Promise<void> {
    if (this.watching) await this.restart();
  }

  async stop(): Promise<void> {
    if (this.quit) return;
    this.quit = true;
    // Finish the current cycle first: the caller stops the VM once this
    // resolves, and a build still staging into it would fail or, worse, stage
    // into a VM that is being reused.
    this.actions.cancelBuild?.();
    await this.cycling?.catch(() => 0);
    const running = this.running;
    this.running = undefined;
    if (running) {
      await this.actions.stopApp();
      await running;
    }
    this.finish();
  }

  private async restart(): Promise<void> {
    if (this.quit) return;
    if (this.busy) {
      // The build under way started before this edit, so run one more after it.
      this.pending = true;
      return;
    }
    await this.cycle(false);
    while (this.pending && !this.quit) {
      this.pending = false;
      await this.cycle(false);
    }
  }

  /** Reports an instance that exited on its own, such as quitting in the VM. */
  private track(launched: Promise<number>): void {
    this.running = launched;
    void launched.then((status) => {
      if (this.running !== launched) return;
      this.running = undefined;
      if (this.quit) return;
      this.status = status;
      if (this.autoQuit) { void this.stop(); return; }
      this.actions.log(`Huterm exited with status ${status}; ${KEY_HELP}`);
    });
  }

  private cycle(first: boolean): Promise<number> {
    const running = this.runCycle(first);
    this.cycling = running;
    return running.finally(() => { if (this.cycling === running) this.cycling = undefined; });
  }

  private async runCycle(first: boolean): Promise<number> {
    this.busy = true;
    try {
      this.actions.log(first ? "building..." : "rebuilding...");
      const built = await this.actions.rebuild();
      if (this.quit) return 0;
      if (built !== 0) {
        // Keep any running instance: a broken build should not close the app.
        this.actions.log(`build failed with status ${built}; keeping the running app`);
        return first ? built : 0;
      }
      if (this.quit) return 0;
      const staged = await this.actions.stage();
      if (staged !== 0) {
        this.actions.log(`staging failed with status ${staged}; keeping the running app`);
        return first ? staged : 0;
      }
      const previous = this.running;
      this.running = undefined;
      if (previous) {
        await this.actions.stopApp();
        await previous;
      }
      if (this.quit) return 0;
      this.track(this.actions.launch());
      this.actions.log(first ? "started" : "relaunched");
      return 0;
    } finally {
      this.busy = false;
    }
  }
}

/** Wires host keystrokes and source watching into a session. I/O glue only. */
export function attachControls(
  session: DevSession,
  paths: string[],
  watch: (path: string, listener: () => void) => { close: () => void },
  debounceMs = 300,
): () => void {
  const report = (error: unknown) => {
    console.error(`dev session: ${error instanceof Error ? error.message : String(error)}`);
  };
  let timer: ReturnType<typeof setTimeout> | undefined;
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(() => { void session.changed().catch(report); }, debounceMs);
  };
  const watchers = paths.map((path) => watch(path, schedule));
  const onData = (data: Buffer) => {
    const key = data.toString();
    // Raw mode delivers the interrupt as a byte rather than a signal.
    // A rejected action must not become an unhandled rejection and kill the run.
    if (key === "\u0003") void session.stop().catch(report);
    else void session.key(key).catch(report);
  };
  const interactive = Boolean(process.stdin.isTTY);
  if (interactive) {
    process.stdin.setRawMode?.(true);
    process.stdin.resume();
    process.stdin.on("data", onData);
  }
  return () => {
    clearTimeout(timer);
    for (const watcher of watchers) watcher.close();
    if (interactive) {
      process.stdin.off("data", onData);
      process.stdin.setRawMode?.(false);
      process.stdin.pause();
    }
  };
}

/**
 * Asks a guest app to exit, then closes its host-side process if the guest
 * agent keeps the exec session open after the process is gone.
 */
export async function stopWithFallback(
  ask: () => void,
  hasExited: () => boolean,
  kill: () => void,
  timeoutMs = 5000,
  pollMs = 100,
): Promise<void> {
  ask();
  const deadline = Date.now() + timeoutMs;
  while (!hasExited() && Date.now() < deadline) await Bun.sleep(pollMs);
  if (!hasExited()) kill();
}
