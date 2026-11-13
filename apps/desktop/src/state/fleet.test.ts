import { describe, expect, test } from "bun:test";
import {
  type FleetEntry,
  fleetNameUpdates,
  fleetSignature,
  knownOnly,
  parseFleetEntry,
  parseFleetView,
  reconcileLocalName,
  viaName,
  wireEntry,
} from "./fleet";

function entry(patch: Partial<FleetEntry> = {}): FleetEntry {
  return {
    hostId: "gw-b",
    name: "Studio",
    nameUpdatedAt: null,
    platform: "macos",
    version: "1.33.0",
    lastSeenAt: "2026-11-13T10:00:00Z",
    via: null,
    ...patch,
  };
}

describe("reading a sync", () => {
  test("an entry needs an id and a name; the rest has defaults", () => {
    expect(parseFleetEntry({ hostId: "gw-b", name: "Studio" })).toEqual({
      hostId: "gw-b",
      name: "Studio",
      nameUpdatedAt: null,
      platform: "unknown",
      version: null,
      lastSeenAt: null,
      via: null,
    });
    expect(parseFleetEntry({ hostId: "gw-b" })).toBeNull();
    expect(parseFleetEntry({ name: "Studio" })).toBeNull();
    expect(parseFleetEntry(null)).toBeNull();
  });

  test("the host may have no id yet, and entries that are not computers are dropped", () => {
    const view = parseFleetView({
      host: { hostId: "", name: "This Mac", platform: "macos" },
      osName: "Lars's MacBook",
      entries: [{ hostId: "gw-b", name: "Studio", via: "gw-c" }, { nope: true }],
    });
    expect(view?.host.hostId).toBe("");
    expect(view?.osName).toBe("Lars's MacBook");
    expect(view?.entries.map((known) => known.hostId)).toEqual(["gw-b"]);
    expect(view?.entries[0]?.via).toBe("gw-c");
    expect(parseFleetView({ entries: [] })).toBeNull();
  });

  test("an entry goes out without a via, which the receiver derives", () => {
    const wire = wireEntry(entry({ via: "gw-c", nameUpdatedAt: "2026-11-13T10:05:00Z" }));
    expect(wire.via).toBeUndefined();
    expect(wire.nameUpdatedAt).toBe("2026-11-13T10:05:00Z");
    expect(wireEntry(entry({ version: null, lastSeenAt: null }))).toEqual({
      hostId: "gw-b",
      name: "Studio",
      platform: "macos",
    });
  });
});

describe("the fleet beside the paired machines", () => {
  const paired = [
    { id: "gw-b", name: "studio.local", displayName: "Studio" },
    { id: "gw-c", name: "mini", displayName: null },
  ];

  test("only computers this one is not paired with are listed as known", () => {
    const entries = [
      entry({ hostId: "gw-d", name: "Raspberry Pi", via: "gw-b" }),
      entry({ hostId: "gw-b" }),
      entry({ hostId: "gw-a", name: "Attic" }),
    ];
    expect(knownOnly(entries, paired).map((known) => known.name)).toEqual(["Attic", "Raspberry Pi"]);
  });

  test("who told us is a paired machine's shown name, else the fleet's own", () => {
    const entries = [entry({ hostId: "gw-e", name: "Far box" })];
    expect(viaName({ via: "gw-b" }, paired, entries)).toBe("Studio");
    expect(viaName({ via: "gw-e" }, paired, entries)).toBe("Far box");
    expect(viaName({ via: "gw-zzz" }, paired, entries)).toBeNull();
    expect(viaName({ via: null }, paired, entries)).toBeNull();
  });

  test("a rename the fleet has is handed to the shell only when it differs from the shown name", () => {
    const entries = [
      entry({ hostId: "gw-b", name: "Mac Studio", nameUpdatedAt: "2026-11-13T10:05:00Z" }),
      // Same as shown: nothing to apply.
      entry({ hostId: "gw-c", name: "mini", nameUpdatedAt: "2026-11-13T10:05:00Z" }),
    ];
    expect(fleetNameUpdates(entries, paired)).toEqual([
      { id: "gw-b", name: "Mac Studio", updatedAt: "2026-11-13T10:05:00Z" },
    ]);
    // A name nobody chose is not a rename: a nickname set before names were shared stays.
    expect(fleetNameUpdates([entry({ hostId: "gw-b", name: "studio.local" })], paired)).toEqual([]);
  });
});

describe("this computer's own name", () => {
  const view = (patch: Partial<FleetEntry> = {}, osName: string | null = "Lars's MacBook") => ({
    host: entry({ hostId: "gw-a", name: "Lars's MacBook", ...patch }),
    osName,
  });

  test("a rename here that is newer than the engine's is pushed", () => {
    const step = reconcileLocalName(
      { displayName: "Desk", nameUpdatedAt: "2026-11-13T10:05:00Z" },
      view({ nameUpdatedAt: "2026-11-13T10:01:00Z" }),
    );
    expect(step.kind).toBe("push");
    if (step.kind === "push") {
      expect(step.entry.name).toBe("Desk");
      expect(step.entry.nameUpdatedAt).toBe("2026-11-13T10:05:00Z");
      expect(step.entry.hostId).toBe("gw-a");
    }
  });

  test("a rename another computer made later is adopted, and not stamped again", () => {
    const step = reconcileLocalName(
      { displayName: "Desk", nameUpdatedAt: "2026-11-13T10:01:00Z" },
      view({ name: "Mac Studio", nameUpdatedAt: "2026-11-13T10:05:00Z" }),
    );
    expect(step).toEqual({ kind: "adopt", name: "Mac Studio", updatedAt: "2026-11-13T10:05:00Z" });
  });

  test("once both agree there is nothing more to do", () => {
    const stamp = "2026-11-13T10:05:00Z";
    expect(
      reconcileLocalName(
        { displayName: "Desk", nameUpdatedAt: stamp },
        view({ name: "Desk", nameUpdatedAt: stamp }),
      ),
    ).toEqual({ kind: "same" });
    // A name from before names were shared is not pushed: it stays on this computer.
    expect(reconcileLocalName({ displayName: "Old nickname", nameUpdatedAt: null }, view())).toEqual({
      kind: "same",
    });
  });

  test("clearing the name returns to the system's, which is pushed like any rename", () => {
    const step = reconcileLocalName(
      { displayName: null, nameUpdatedAt: "2026-11-13T10:09:00Z" },
      view({ name: "Desk", nameUpdatedAt: "2026-11-13T10:05:00Z" }),
    );
    expect(step.kind).toBe("push");
    if (step.kind === "push") expect(step.entry.name).toBe("Lars's MacBook");
    // Adopting the system's own name shows as no name.
    expect(
      reconcileLocalName(
        { displayName: "Desk", nameUpdatedAt: "2026-11-13T10:01:00Z" },
        view({ name: "Lars's MacBook", nameUpdatedAt: "2026-11-13T10:09:00Z" }),
      ),
    ).toEqual({ kind: "adopt", name: null, updatedAt: "2026-11-13T10:09:00Z" });
  });

  test("a computer no gateway ever ran on has no id to push under", () => {
    expect(
      reconcileLocalName(
        { displayName: "Desk", nameUpdatedAt: "2026-11-13T10:05:00Z" },
        view({ hostId: "" }),
      ),
    ).toEqual({ kind: "same" });
  });
});

describe("when a sync is due", () => {
  const local = { displayName: null, nameUpdatedAt: null };
  const connected = [{ id: "gw-b", displayName: null, state: "connected" as const }];

  test("a rename or a link coming up or down changes the signature; a retry countdown does not", () => {
    const base = fleetSignature(local, connected);
    expect(
      fleetSignature({ displayName: "Desk", nameUpdatedAt: "2026-11-13T10:05:00Z" }, connected),
    ).not.toBe(base);
    expect(fleetSignature(local, [{ ...connected[0]!, displayName: "Mac Studio" }])).not.toBe(base);
    expect(fleetSignature(local, [{ ...connected[0]!, state: "offline" as const }])).not.toBe(base);
    expect(fleetSignature(local, [{ ...connected[0]!, state: "connected" as const }])).toBe(base);
  });
});
