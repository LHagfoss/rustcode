import { type Json, type JsonObject, str } from "./jsonrpc";
import { ACCESS_LEVELS } from "./remote";

/**
 * The mock's `_rustcode/remote/access`: sets a paired device's ACP ceiling in place, as the
 * gateway does. Kept out of acp/mock.ts, which is at its size cap.
 */
export function setMockAccess(devices: JsonObject[], params: JsonObject): Json {
  const deviceId = str(params.deviceId) ?? "";
  const access = ACCESS_LEVELS.find((level) => level === str(params.access));
  const device = devices.find((entry) => entry.id === deviceId);
  if (!access)
    throw Object.assign(new Error(`unknown access level '${str(params.access)}'`), { code: -32602 });
  if (!device) throw Object.assign(new Error(`no paired device matches ${deviceId}`), { code: -32602 });
  device.access = access;
  return { deviceId, access };
}
