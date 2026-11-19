import { describe, expect, test } from "bun:test";
import { parseHubStatus } from "./hub";

describe("parseHubStatus", () => {
  test("reads the role, the membership and each member", () => {
    const status = parseHubStatus({
      hub: true,
      member: { machine: "vps-id", name: "vps", access: "delegate", ask: false },
      members: [
        {
          host_id: "a",
          name: "laptop",
          access: "manage",
          ask: true,
          online: false,
          last_seen_at: "2026-11-18T21:04:00Z",
          queued: 2,
        },
        { name: "no id" },
      ],
    });
    expect(status).toEqual({
      hub: true,
      member: { machine: "vps-id", name: "vps", access: "delegate", ask: false },
      members: [
        {
          hostId: "a",
          name: "laptop",
          access: "manage",
          ask: true,
          online: false,
          lastSeenAt: "2026-11-18T21:04:00Z",
          queued: 2,
        },
      ],
    });
  });

  test("no role, no member, and a gateway that does not answer", () => {
    expect(parseHubStatus({ hub: false, member: null, members: null })).toEqual({
      hub: false,
      member: null,
      members: null,
    });
    expect(parseHubStatus("junk")).toEqual({ hub: false, member: null, members: null });
  });

  test("an unknown access level reads as read-only", () => {
    const status = parseHubStatus({ member: { machine: "m", name: "m", access: "root" } });
    expect(status.member?.access).toBe("read_only");
    expect(status.member?.ask).toBe(true);
  });
});
