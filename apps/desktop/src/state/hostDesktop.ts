/**
 * Cross-machine desktop control: routes a thread's `computer_*` call with a
 * `machine` argument to this computer's local engine (`_rustcode/desktop/call`)
 * after showing the approval dialog right here on the controlled computer.
 */

import { type JsonObject, obj, str } from "../acp/jsonrpc";
import { Method } from "../acp/types";
import { refreshGrants } from "./approvals";
import { askUserGrant, droppable, trackCall } from "./host";
import { checkedMachine, localMachine, machineFields, machineOfHost, requestOn } from "./hostMachines";
import { failure, failureJson, isFailure, type RuleThread } from "./orchestration";
import type { ComputerTool } from "./orchestrationCalls";
import { errorText } from "./store";
import { LOCAL_HOST } from "./threadKeys";

const DESKTOP_CALL_TIMEOUT_MS = 60_000;
const MAX_APPROVAL_ROUNDS = 2;

/** Format one desktop tool call for the approval dialog, with typed text redacted. */
export function desktopApprovalDetail(tool: string, args: JsonObject): string {
  const purpose = str(args.purpose)?.trim();
  const preview: JsonObject = {};
  for (const [key, value] of Object.entries(args)) {
    if (key === "machine" || key === "purpose") continue;
    preview[key] = key === "text" ? "[redacted]" : value;
  }
  const summary = `${tool} ${JSON.stringify(preview)}`;
  return purpose ? `${purpose}\n${summary}` : summary;
}

/**
 * Run one `computer_*` tool call on this computer's local engine. Calls
 * targeting any machine other than the computer running this RustCode app are
 * refused so approval always happens on the controlled desktop itself.
 */
export async function runDesktopControl(
  caller: RuleThread,
  call: { desktopTool: ComputerTool; args: JsonObject; machine?: string; callId?: string },
  callerHost: string,
): Promise<JsonObject> {
  const machine = checkedMachine(call.machine, callerHost);
  if (isFailure(machine)) return failureJson(machine);
  if (machine.host !== LOCAL_HOST) {
    return failureJson(
      failure(
        "not_allowed",
        `Desktop control only runs on this computer (${localMachine().name}), where the RustCode app is open to approve it. Open RustCode on ${machine.name} to control its desktop.`,
      ),
    );
  }
  const tracked = call.callId ? trackCall(call.callId, caller.id) : undefined;
  const callerMachine = callerHost ? machineOfHost(callerHost).name : undefined;
  const basePayload: JsonObject = {
    sessionId: caller.id,
    tool: call.desktopTool,
    args: call.args,
    ...(callerMachine ? { callerMachine } : {}),
  };
  try {
    let answer = await requestOn<JsonObject>(
      LOCAL_HOST,
      Method.desktopCall,
      basePayload,
      DESKTOP_CALL_TIMEOUT_MS,
    );
    for (let round = 0; round < MAX_APPROVAL_ROUNDS; round++) {
      if (answer.status === "ok") {
        const result = obj(answer.result) ?? {};
        return { ...result, ...machineFields(machine) };
      }
      if (answer.status !== "needs_approval") {
        return failureJson(failure("failed", "unexpected response from desktop control"));
      }
      if (tracked?.dropped) {
        return failureJson(failure("cancelled", "the desktop control call was cancelled before approval"));
      }
      const app = obj(answer.app);
      const appKey = str(app?.key) ?? "";
      const appName = str(app?.name) ?? "this app";
      const sensitive = app?.sensitive === true;
      const asked = askUserGrant(
        {
          kind: "desktop",
          caller: caller.id,
          callerTitle: caller.title,
          target: appName,
          detail: desktopApprovalDetail(call.desktopTool, call.args),
          onceOnly: sensitive,
          sensitive,
          ...(callerMachine ? { fromMachine: callerMachine } : {}),
        },
        tracked,
      );
      if (isFailure(asked)) return failureJson(asked);
      const choice = await asked;
      if (tracked?.dropped) {
        return failureJson(failure("cancelled", "the desktop control call was cancelled before approval"));
      }
      if (choice === "deny") {
        return failureJson(
          failure(
            "denied",
            `app_denied: the user did not allow controlling ${appName}. Ask them what to do instead.`,
          ),
        );
      }
      const approval = choice === "always" && !sensitive ? "app" : "once";
      answer = await requestOn<JsonObject>(
        LOCAL_HOST,
        Method.desktopCall,
        {
          ...basePayload,
          approval,
          approvedApp: appKey,
        },
        DESKTOP_CALL_TIMEOUT_MS,
      );
      if (approval === "app" && answer.status === "ok") void refreshGrants();
    }
    if (answer.status === "ok") {
      const result = obj(answer.result) ?? {};
      return { ...result, ...machineFields(machine) };
    }
    return failureJson(failure("conflict", "the target app changed before approval completed; try again"));
  } catch (error) {
    return failureJson(failure("failed", errorText(error)));
  } finally {
    if (call.callId) droppable.delete(call.callId);
  }
}
