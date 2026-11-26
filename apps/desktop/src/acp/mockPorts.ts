import { type JsonObject, num, str } from "./jsonrpc";

type MockPort = {
  port: number;
  pid: number;
  address: string;
  processName: string;
  label?: string;
  url: string;
};

const mockPortsByCwd = new Map<string, MockPort[]>();

/** Handle `_rustcode/ports/list` and `_rustcode/ports/kill` in mock transport mode. */
export function mockPortsCall(method: string, params: JsonObject): JsonObject {
  const cwd = str(params.cwd) ?? "";
  const current = mockPortsByCwd.get(cwd) ?? [];
  if (method === "_rustcode/ports/kill") {
    const pid = num(params.pid);
    const port = num(params.port);
    const next = current.filter((item) => !(item.pid === pid && item.port === port));
    mockPortsByCwd.set(cwd, next);
    return { list: { cwd, ports: next } };
  }
  return { list: { cwd, ports: current } };
}
