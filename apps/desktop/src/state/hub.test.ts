import { describe, expect, test } from "bun:test";
import {
  HUB_UNREAD,
  type HubState,
  type HubStatus,
  hubChanging,
  hubError,
  hubStatus,
  parseHubStatus,
  transitionHub,
} from "./hub";

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

describe("the hub role state machine", () => {
  const off: HubStatus = { hub: false, member: null, members: null };
  const on: HubStatus = { hub: true, member: null, members: [] };
  const read: HubState = transitionHub(HUB_UNREAD, { type: "read", epoch: 0, status: off });

  test("a first read shows the role, a failed one says why", () => {
    expect(hubStatus(read)).toEqual(off);
    const failed = transitionHub(HUB_UNREAD, { type: "readFailed", epoch: 0, error: "no answer" });
    expect(hubError(failed)).toBe("no answer");
    expect(hubStatus(failed)).toBeNull();
    expect(hubStatus(transitionHub(failed, { type: "read", epoch: 0, status: off }))).toEqual(off);
  });

  test("a failed read never replaces a role on screen", () => {
    const after = transitionHub(read, { type: "readFailed", epoch: 0, error: "no answer" });
    expect(after).toBe(read);
    expect(hubError(after)).toBeNull();
  });

  test("a change needs a role, and one runs at a time", () => {
    expect(transitionHub(HUB_UNREAD, { type: "change" })).toBe(HUB_UNREAD);
    const changing = transitionHub(read, { type: "change" });
    expect(hubChanging(changing)).toBe(true);
    expect(hubStatus(changing)).toEqual(off);
    expect(transitionHub(changing, { type: "change" })).toBe(changing);
  });

  test("a change ends with the new role, or with the old one when it failed", () => {
    const changing = transitionHub(read, { type: "change" });
    const changed = transitionHub(changing, { type: "changed", status: on });
    expect(hubStatus(changed)).toEqual(on);
    expect(hubChanging(changed)).toBe(false);
    const failed = transitionHub(changing, { type: "changeFailed" });
    expect(hubStatus(failed)).toEqual(off);
    expect(hubChanging(failed)).toBe(false);
  });

  test("a read that left before a change does not bring the old role back", () => {
    const leftAt = read.epoch;
    const changing = transitionHub(read, { type: "change" });
    expect(transitionHub(changing, { type: "read", epoch: leftAt, status: off })).toBe(changing);
    const changed = transitionHub(changing, { type: "changed", status: on });
    expect(transitionHub(changed, { type: "read", epoch: leftAt, status: off })).toBe(changed);
  });

  test("a read that left during a change is dropped, the next one counts", () => {
    const changing = transitionHub(read, { type: "change" });
    const changed = transitionHub(changing, { type: "changed", status: on });
    expect(transitionHub(changed, { type: "read", epoch: changing.epoch, status: off })).toBe(changed);
    const later = transitionHub(changed, { type: "read", epoch: changed.epoch, status: off });
    expect(hubStatus(later)).toEqual(off);
  });
});
