// design-lint: allow-file (fake colours and issue numbers in canned data)
import type { JsonObject } from "./jsonrpc";
import { MockEngine } from "./mock";
import type { MockOptions } from "./mockFixtures";
import { mockGitChanges } from "./mockGit";
import {
  mockDiscovery,
  mockIsPublic,
  mockMachine,
  mockMachineList,
  mockServerService,
} from "./mockMachineFixtures";
import { mockRouteCommand } from "./mockMachineRoutes";
import { mockLinkMeta } from "./mockSmartUi";
import type { Transport, TransportEvent } from "./transport";

// The mock transport (`MockTransport`) the app talks to under VITE_TRANSPORT=mock, and the
// fixtures only it answers with. Split out of mock.ts.

export class MockTransport implements Transport {
  readonly name = "mock";
  private engine: MockEngine | null = null;
  private onEvent: ((event: TransportEvent) => void) | null = null;
  /** The machines this app is paired with, as `machines_list` reports them. */
  private machineList: JsonObject[];
  private sshRunning = false;
  private sshSteps: JsonObject[] = [];

  /** Whether the RustCode server is installed as a service (`?machines=connect` starts without it). */
  private serverInstalled: boolean;

  private crashReportsEnabled = true;

  /** `<config>/hub.json` as `machines hub status --json` answers it. */
  private hub: { hub: boolean; member: JsonObject | null } = { hub: false, member: null };

  private hubStatus(): JsonObject {
    const at = new Date(Date.now() - 3 * 3600_000).toISOString();
    return {
      hub: this.hub.hub,
      member: this.hub.member,
      members: this.hub.hub
        ? [
            {
              host_id: "mock-laptop",
              name: "laptop",
              access: "delegate",
              ask: true,
              online: true,
              last_seen_at: new Date().toISOString(),
              queued: 0,
            },
            {
              host_id: "mock-desk",
              name: "desk",
              access: "read_only",
              ask: false,
              online: false,
              last_seen_at: at,
              queued: 2,
            },
          ]
        : null,
    };
  }

  constructor(private readonly options: MockOptions = {}) {
    this.machineList = mockMachineList(options.machines);
    this.serverInstalled = options.machines !== "connect";
  }

  async start(onEvent: (event: TransportEvent) => void): Promise<void> {
    this.onEvent = onEvent;
    this.engine = new MockEngine(
      (line) => queueMicrotask(() => onEvent({ type: "line", line })),
      this.options,
    );
    if (this.options.bootMs) await new Promise((r) => setTimeout(r, this.options.bootMs));
    this.startedAt = Date.now();
    for (const line of MOCK_SERVER_LOG) onEvent({ type: "log", line });
    onEvent({ type: "open", engine: "mock" });
    const crash = this.options.crashAfterMs;
    if (crash && !this.crashed) {
      this.crashed = true;
      setTimeout(() => {
        const reason = "The server was stopped by signal 9.";
        this.lastExit = { code: null, signal: 9, atMs: Date.now(), reason, expected: false };
        this.crashRestarts += 1;
        onEvent({ type: "closed", reason });
        void this.start(onEvent);
      }, crash);
    }
  }
  private crashed = false;
  /** What Settings › Diagnostics reads of the process (`server_status`). */
  private startedAt = Date.now();
  private crashRestarts = 0;
  private lastExit: JsonObject | null = null;

  async restart(): Promise<void> {
    const onEvent = this.onEvent;
    if (!onEvent) return;
    const reason = "The server exited (code 0).";
    this.lastExit = { code: 0, signal: null, atMs: Date.now(), reason, expected: true };
    onEvent({ type: "closed", reason });
    await this.start(onEvent);
  }

  /** `server_status`, as src-tauri/src/diagnostics.rs answers it. */
  private serverStatus(): JsonObject {
    return {
      running: this.engine !== null,
      pid: this.engine ? 48213 : null,
      path: "/Applications/RustCode.app/Contents/MacOS/rustcode",
      startedAtMs: this.engine ? this.startedAt : null,
      crashRestarts: this.crashRestarts,
      lastExit: this.lastExit,
      version: "1.21.0",
      configDir: "/Users/lars/.config/rustcode",
      logFile: "/Users/lars/.config/rustcode/debug.log",
    };
  }

  send(line: string): void {
    this.engine?.receive(line);
  }

  async call(command: string, args: Record<string, unknown>): Promise<unknown> {
    if (command === "machines_list") return this.machineList;
    if (command === "machines_discover") return mockDiscovery(this.machineList);
    if (command === "machines_ask") {
      const address = String(args.address ?? "");
      if (!address) throw new Error("Enter a machine address first.");
      if (mockIsPublic(address))
        throw new Error(
          `refusing to dial ${address}: it is a public address and the gateway has no TLS listener, so the pairing code and the device token would travel in the clear. Dial a LAN, Tailscale or VPN address instead; encrypted transport is tracked in #2651`,
        );
      // Asked, then accepted on the machine: the code is what comes next. The
      // ask id is the page's own, and it names the request for `machines_pair`.
      return { askId: String(args.askId ?? ""), codeLength: 8, expiresInSecs: 120 };
    }
    if (command === "machines_cancel") return null;
    if (command === "machines_pair") {
      const code = String(args.code ?? "");
      if (!/^\d{8}$/.test(code))
        throw new Error("A pairing code is exactly 8 digits. Ask the machine again for a new code.");
      const paired = mockMachine("connected", Date.now());
      this.machineList = [...this.machineList.filter((machine) => machine.id !== paired.id), paired];
      return { ...paired, state: "connecting" };
    }
    if (command === "machines_retry") {
      // Nothing waits on a backoff in the mock: no machine is woken.
      return [];
    }
    if (command === "machines_connect_code") {
      const code = String(args.code ?? "").trim();
      const short = code.replace(/[-\s]/g, "").toUpperCase();
      if (!code.toLowerCase().startsWith("rustcode://connect/") && !/^[2-9A-HJ-NP-Z]{16}$/.test(short))
        throw new Error("a connect code has 16 characters, like K7MQ-29XH-TP4W-NRD6");
      const paired = mockMachine("connected", Date.now());
      this.machineList = [...this.machineList.filter((machine) => machine.id !== paired.id), paired];
      return { ...paired, state: "connecting" };
    }
    if (command === "machines_ssh_progress") return { running: this.sshRunning, steps: this.sshSteps };
    if (command === "machines_connect_ssh") {
      // Steps tick off over a few seconds. A host with "denied" in it fails to sign in, one with
      // "novpn" is reached over SSH but not on RustCode's port.
      const target = String(args.target ?? "").trim();
      const host = target.includes("@") ? target.slice(target.lastIndexOf("@") + 1) : target;
      const labels: Record<string, string> = {
        sign_in: `Signing in to ${host}`,
        check: `Checking RustCode on ${host}`,
        install: `Installing RustCode 1.31.4 on ${host}`,
        start: `Starting the server on ${host}`,
        pair: `Pairing with ${host}`,
        pair_back: `Letting ${host} reach this computer`,
      };
      this.sshRunning = true;
      this.sshSteps = [];
      const set = (step: string, state: string, detail: string | null = null) => {
        this.sshSteps = [
          ...this.sshSteps.filter((seen) => seen.step !== step),
          { step, state, label: labels[step] ?? step, detail },
        ];
      };
      const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 700));
      try {
        set("sign_in", "running");
        await tick();
        if (target.includes("denied")) {
          const message = `SSH could not sign in to ${host} without asking for a password or passphrase. Add your key to the agent (\`ssh-add\`), or connect once in a terminal, then try again.`;
          set("sign_in", "failed", message);
          throw new Error(message);
        }
        set("sign_in", "done");
        set("check", "running");
        await tick();
        set("check", "done", "RustCode is not installed");
        if (args.allowInstall === false) {
          const message = `RustCode is not installed on ${host}. Install it there, or allow installing.`;
          set("install", "failed", message);
          throw new Error(message);
        }
        set("install", "running");
        await tick();
        set("install", "done", "/home/lars/.rustcode/bin/rustcode");
        set("start", "running");
        await tick();
        set("start", "done");
        set("pair", "running");
        await tick();
        if (target.includes("novpn")) {
          const message = `${host} is reachable over SSH, but not on RustCode's port (203.0.113.7:17879). Put both computers on the same network or a VPN such as NetBird or Tailscale, then try again.`;
          set("pair", "failed", message);
          throw new Error(message);
        }
        set("pair", "done", "100.92.13.44:17879");
        const paired = mockMachine("connected", Date.now());
        this.machineList = [...this.machineList.filter((machine) => machine.id !== paired.id), paired];
        return { ...paired, state: "connecting" };
      } finally {
        this.sshRunning = false;
      }
    }
    if (command === "machines_pair_back") {
      // A host with "noback" in it cannot reach this computer, so pairing stays one way.
      const target = String(args.target ?? "").trim();
      const host = target.includes("@") ? target.slice(target.lastIndexOf("@") + 1) : target;
      this.sshRunning = true;
      const label = `Letting ${host} reach this computer`;
      const set = (state: string, detail: string | null = null) => {
        this.sshSteps = [
          ...this.sshSteps.filter((seen) => seen.step !== "pair_back"),
          { step: "pair_back", state, label, detail },
        ];
      };
      try {
        set("running");
        await new Promise<void>((resolve) => setTimeout(resolve, 700));
        if (target.includes("noback")) {
          const message = `${host} could not reach this computer on RustCode's port: 192.168.1.20:17879 timed out. Put both computers on the same network or a VPN such as NetBird or Tailscale, and check that nothing blocks the port here.`;
          set("failed", message);
          throw new Error(message);
        }
        set("done");
        return null;
      } finally {
        this.sshRunning = false;
      }
    }
    if (command === "machines_probe") {
      // The LAN answers; an overlay that accepts no inbound connections times out.
      const at = new Date().toISOString();
      this.machineList = this.machineList.map((machine) => {
        if (args.id && machine.id !== String(args.id)) return machine;
        const routes = (machine.routes as JsonObject[]).map((route) => ({
          ...route,
          probe:
            route.kind === "lan"
              ? { outcome: "reachable", latencyMs: 3, detail: null, at }
              : { outcome: "timeout", latencyMs: null, detail: null, at },
        }));
        return { ...machine, routes };
      });
      return this.machineList;
    }
    if (command === "app_version") return "1.28.0";
    if (command === "server_service_status") return mockServerService(this.serverInstalled);
    if (command === "server_service_install") {
      this.serverInstalled = true;
      return mockServerService(true);
    }
    if (command === "server_link_path") return mockServerService(this.serverInstalled);
    if (command === "hub_status") return this.hubStatus();
    if (command === "hub_set") {
      if (args.action === "on" || args.action === "off") this.hub.hub = args.action === "on";
      if (args.action === "leave") this.hub.member = null;
      if (args.action === "join") {
        const machine = this.machineList.find((m) => m.id === String(args.machine));
        if (!machine) throw new Error(`no saved machine matches ${String(args.machine)}`);
        this.hub.member = {
          machine: String(machine.id),
          name: String(machine.name),
          access: String(args.access ?? "read_only"),
          ask: args.ask !== false,
        };
      }
      return this.hubStatus();
    }
    if (
      command === "machines_remove" ||
      command === "machines_forward_take_over" ||
      command.startsWith("machines_route_")
    ) {
      this.machineList = mockRouteCommand(command, args, this.machineList);
      return command === "machines_remove" ? null : this.machineList;
    }
    if (command === "machines_set_paused") {
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, paused: args.paused === true } : machine,
      );
      return null;
    }
    if (command === "machines_set_display_name") {
      const name = typeof args.name === "string" && args.name.trim() ? args.name.trim() : null;
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, displayName: name } : machine,
      );
      return null;
    }
    if (command === "machines_set_icon") {
      const icon = typeof args.icon === "string" && args.icon ? args.icon : null;
      const tint = icon && typeof args.tint === "string" && args.tint ? args.tint : null;
      this.machineList = this.machineList.map((machine) =>
        machine.id === String(args.id) ? { ...machine, icon, iconTint: tint } : machine,
      );
      return null;
    }
    // A machine's own channel is opened in process (`MachineTransport` hands it a MockMachine).
    if (command === "machine_connect" || command === "machine_send" || command === "machine_disconnect") {
      const machine = this.machineList.find((saved) => saved.id === String(args.id));
      return { connected: machine?.state !== "offline" && machine?.state !== "needsPairing" };
    }
    if (command === "link_meta") return mockLinkMeta(String(args.url));
    if (command === "server_status") return this.serverStatus();
    if (command === "system_info") return MOCK_SYSTEM_INFO;
    // Settings › About › System setup: a finished install, one step not needed.
    if (command === "system_setup_status" || command === "system_setup_run") return MOCK_SYSTEM_SETUP;
    if (command === "resolve_binaries")
      return (args.names as string[]).map((name) =>
        ["codex", "claude-agent-acp", "gemini", "opencode"].includes(name)
          ? `/opt/homebrew/bin/${name}`
          : null,
      );
    if (command === "logs_folder_open") return null;
    if (command === "crash_report_status") return { enabled: this.crashReportsEnabled, configured: false };
    if (command === "crash_report_set_enabled") {
      this.crashReportsEnabled = args.enabled !== false;
      return { enabled: this.crashReportsEnabled, configured: false };
    }
    if (command === "crash_report_send") return null;
    if (command === "diagnostics_save") return `/Users/lars/Downloads/${String(args.name)}`;
    if (command === "thread_log_open") {
      const id = String(args.id);
      if (!/^[A-Za-z0-9_-]+$/.test(id)) throw new Error("Not a thread id.");
      return `/Users/lars/.config/rustcode/threads/2026/11/08/${id}/logs/debug.log`;
    }
    if (command === "thread_log_tail")
      return "2026-11-08T10:00:00Z INFO turn started\n2026-11-08T10:00:04Z WARN /Users/lars/code/app: request failed (HTTP 429)";
    if (command === "thread_rename")
      return this.engine?.renameThread(String(args.id), String(args.title), args.user === true);
    if (command === "thread_user_titled") return this.engine?.userTitled.has(String(args.id)) ?? false;
    if (command === "thread_trash") return this.engine?.trashThread(String(args.id));
    if (command === "project_files") return MOCK_PROJECT_FILES;
    if (command === "git_changes") {
      const changes = mockGitChanges(String(args.cwd), args.base ? String(args.base) : undefined);
      // The summary the panel's counts read leaves the diff body out, as the shell does.
      return args.summary ? { ...changes, diff: "" } : changes;
    }
    if (
      command === "reveal_path" ||
      command === "open_terminal_at" ||
      command === "open_path" ||
      command === "open_in_editor"
    )
      return null;
    if (command === "thread_image") {
      // Canned fixtures only: the actual filesystem policy lives in native.rs.
      const path = String(args.path);
      if (path === "/Users/you/Private/screenshot.png")
        throw new Error("Outside the thread, project, worktree and temporary folders.");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="480" height="200" viewBox="0 0 480 200"><rect width="480" height="200" rx="12" fill="#18181a"/><polyline fill="none" stroke="#93a3e8" stroke-width="3" points="20,180 80,170 140,150 200,110 260,40 320,40 380,40 440,40"/><text x="20" y="28" fill="#a1a1a1" font-family="-apple-system" font-size="14">${path}</text></svg>`;
      return { name: path.split("/").pop(), mimeType: "image/svg+xml", data: btoa(svg) };
    }
    if (command === "attached_files") {
      // The shell's `attached_files` (native.rs), with the same rules: inside the roots,
      // a file, at most ten of them and 5 MB each. A canned size stands in for the file.
      const paths = Array.isArray(args.paths) ? args.paths.map(String) : [];
      const absolute = (path: string) => /^(?:[A-Za-z]:)?[\\/]/.test(path);
      const one = (path: string) => path.replace(/\\/g, "/");
      const roots = Array.isArray(args.roots) ? args.roots.map(String).filter(absolute) : [];
      if (paths.length > 10) throw new Error("A message attaches at most 10 files.");
      if (!roots.length) throw new Error("This thread has no folder to attach a file from.");
      return paths.map((path) => {
        if (!absolute(path)) throw new Error(`${path} is not an absolute path.`);
        const file = one(path);
        const root = roots.find((r) => {
          const base = one(r).replace(/\/+$/, "");
          return file === base || file.startsWith(`${base}/`);
        });
        if (!root) throw new Error(`${path} is outside this thread's project.`);
        return { path, name: file.split("/").pop() ?? path, bytes: 1024 };
      });
    }
    if (command === "skill_create") return `~/.config/rustcode/skills/${String(args.name)}/SKILL.md`;
    // The mock window has no system clipboard to paste from, and no picture to stand in for.
    if (command === "clipboard_image") return null;
    // A dropped path is a folder unless it has an extension; nothing on the mock's disk is missing.
    if (command === "path_kinds") {
      const paths = Array.isArray(args.paths) ? args.paths.map(String) : [];
      return paths.map((path) =>
        !path.startsWith("/") ? "missing" : /\.[a-z0-9]+$/i.test(path) ? "file" : "folder",
      );
    }
    throw new Error(`The mock does not run ${command}.`);
  }

  async stop(): Promise<void> {
    this.engine = null;
  }
}

/** The server's stderr at start, a few levels of it, for Settings › Diagnostics. */
const MOCK_SERVER_LOG = [
  "desktop: started /Applications/RustCode.app/Contents/MacOS/rustcode --acp (pid 48213)",
  "2026-11-08T09:12:01.114Z  INFO rustcode::acp: listening on stdio (protocol 1)",
  "2026-11-08T09:12:01.120Z DEBUG rustcode::config: read /Users/lars/.config/rustcode/config.toml",
  "2026-11-08T09:12:01.402Z  INFO rustcode::harness: found codex 0.50.0, claude-agent-acp 0.9.1",
  "2026-11-08T09:12:01.733Z  WARN rustcode::harness: gemini: not signed in (gemini auth login)",
  '2026-11-08T09:12:02.010Z ERROR rustcode::mcp: server "linear" exited during initialize: token=lin_api_abcdef123456',
  "2026-11-08T09:12:02.011Z  INFO rustcode::store: data.sqlite ready (migrations 14)",
];

/** `system_info`, as the shell answers it on a Mac. */
const MOCK_SYSTEM_INFO: JsonObject = {
  os: "macos",
  osVersion: "15.2 (24C101)",
  kernel: null,
  arch: "aarch64",
  appVersion: "1.21.0",
  tauriVersion: "2.12.5",
  webview: "WKWebView",
  webviewVersion: "20620.1.16.11.8",
  home: "/Users/lars",
  graphicsEnv: [],
};

/**
 * A project's files as the shell lists them (git's own order): more than the `@` drawer shows at
 * once, so its short list and its `Show all` row can be seen in the mock window.
 */
const MOCK_PROJECT_FILES = [
  "Cargo.lock",
  "Cargo.toml",
  "README.md",
  "apps/desktop/package.json",
  "apps/desktop/src/lib/native.ts",
  "apps/desktop/src/main.tsx",
  "apps/desktop/src/state/compose.ts",
  "apps/desktop/src/state/store.ts",
  "apps/desktop/src/ui/Composer.tsx",
  "apps/desktop/src/ui/Sidebar.tsx",
  "docs/design.md",
  "docs/desktop.md",
  "docs/my notes.md",
  "rustcode/engine/src/acp.rs",
  "rustcode/engine/src/tools/mod.rs",
  "scripts/release.sh",
];

/** Settings › About › System setup in the mock: the macOS steps of a finished install. */
const MOCK_SYSTEM_SETUP = [
  {
    id: "quarantine",
    title: "Clear the download quarantine flag",
    about:
      "macOS marks a downloaded app as quarantined, which can make its command line prompt or fail the first time. This clears the flag on RustCode.",
    auto: true,
    status: "done",
    message: "No quarantine flag on RustCode.",
    lastRun: "2026-11-14T09:00:00Z",
  },
  {
    id: "signature",
    title: "Keep one signature across updates",
    about:
      "Releases are signed ad hoc, so macOS sees every update as a new app and asks again for Keychain access and Background Items. A local certificate gives updates one identity.",
    auto: true,
    status: "done",
    message: "RustCode has a signature that stays the same across updates.",
    lastRun: "2026-11-14T09:00:00Z",
  },
  {
    id: "cli-link",
    title: "Put `rustcode` on your PATH",
    about:
      "Links `rustcode` into ~/.local/bin when no copy is on your PATH, so a terminal runs the same version as the app.",
    auto: true,
    status: "done",
    message: "`rustcode` is at /Users/lars/.cargo/bin/rustcode.",
    lastRun: "2026-11-14T09:00:00Z",
  },
  {
    id: "shell-path",
    title: "Check ~/.local/bin is on your shell's PATH",
    about:
      "Reads your login shell's PATH. RustCode never edits your shell profile; when the folder is missing it shows the line to add.",
    auto: true,
    status: "skipped",
    message: "`rustcode` is not linked into ~/.local/bin, so there is nothing to add.",
    lastRun: "2026-11-14T09:00:00Z",
  },
  {
    id: "background-server",
    title: "Run the RustCode server in the background",
    about:
      "Installs the server as a LaunchAgent, systemd user unit or logon task, so paired machines reach this computer with the app closed. See Machines.",
    auto: false,
    status: "done",
    message: "The server starts when you log in.",
    lastRun: "2026-11-14T09:00:00Z",
  },
];
