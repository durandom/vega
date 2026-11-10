// Managed by vega config (pi.dcg); upstream v0.16.0 docs/pi-integration.md.
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
// dcg-guard.ts — block destructive shell commands with dcg
// https://github.com/Dicklesworthstone/destructive_command_guard
import { spawn } from "node:child_process";

const DCG_BIN = process.env.DCG_BIN ?? "dcg";

// Fail closed on unavailable, indeterminate, or invalid-config results.
const UNAVAILABLE = { deny: true, reason: "dcg could not verify this command; run dcg doctor --strict." };

function dcgDecision(command: string): Promise<{ deny: boolean; reason: string }> {
  return new Promise((resolve) => {
    // Resolve exactly once: "error" and "close" can both fire, and a
    // synchronous spawn failure must not race a later event.
    let settled = false;
    const settle = (decision: { deny: boolean; reason: string }) => {
      if (settled) return;
      settled = true;
      resolve(decision);
    };

    // Pi runs on Bun, and Bun's spawn() differs from Node in two ways that
    // matter here: a failed posix_spawn (ENOENT, ENFILE, EAGAIN, ...) is
    // thrown synchronously from spawn() rather than delivered as an "error"
    // event, and when the stdio pipes cannot be set up the returned child can
    // have no `stdout` at all. Handle both, or the extension crashes and the
    // host aborts the tool call instead of failing closed.
    let child;
    try {
      child = spawn(DCG_BIN, ["--robot", "test", "--dialect", "posix", command], {
        stdio: ["ignore", "pipe", "ignore"],
      });
    } catch {
      settle(UNAVAILABLE);
      return;
    }

    // Bound a stuck or unexpectedly noisy guard without allowing the tool call.
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      settle(UNAVAILABLE);
    }, 5000);
    child.on("close", () => clearTimeout(timer));
    let stdout = "";
    child.stdout?.on("data", (chunk) => {
      if (stdout.length < 65536) stdout += chunk.toString().slice(0, 65536 - stdout.length);
    });

    child.on("error", () => settle(UNAVAILABLE));

    child.on("close", (code) => {
      if (code === 1) {
        // Denied. The reason lives in the robot-mode JSON; if the stdout
        // pipe was missing the exit code is still authoritative.
        let reason = "Blocked by dcg (destructive command).";
        try {
          const parsed = JSON.parse(stdout);
          if (parsed?.reason) reason = parsed.reason;
          if (parsed?.rule_id) reason += ` [${parsed.rule_id}]`;
        } catch {
          /* keep the default reason */
        }
        settle({ deny: true, reason });
      } else if (code === 0) {
        settle({ deny: false, reason: "" });
      } else {
        // Nonzero without a deny verdict is incomplete verification.
        settle(UNAVAILABLE);
      }
    });
  });
}

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event) => {
    if (event.toolName !== "bash") return;
    const command = String(event.input?.command ?? "");
    if (!command.trim()) return;

    let decision;
    try {
      decision = await dcgDecision(command);
    } catch {
      // Unexpected guard failures also block until the guard is healthy.
      decision = UNAVAILABLE;
    }
    if (decision.deny) {
      return { block: true, reason: decision.reason };
    }
  });
}
