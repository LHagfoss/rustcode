import { describe, expect, test } from "bun:test";
import { threadLineage } from "./lineage";
import { type Link, type LinkState, MAX_DELEGATES_PER_THREAD } from "./orchestration";

const link = (n: number, change: Partial<Link>): Link => ({
  id: `a/${n}`,
  orchestrator: "a",
  target: "me",
  orchestratorTitle: "Parent",
  targetTitle: "Me",
  message: "Do it",
  state: "done" as LinkState,
  createdAt: n,
  ...change,
});

describe("a thread's lineage", () => {
  test("a started link names the parent", () => {
    const { parent, messagedBy } = threadLineage("me", [
      link(1, { started: true, state: "working", createdAt: 30 }),
    ]);
    expect(parent).toMatchObject({ id: "a", title: "Parent", live: true, started: true });
    expect(parent?.link?.id).toBe("a/1");
    expect(messagedBy).toBeUndefined();
  });

  test("a pruned started link leaves the parent the thread recorded", () => {
    const { parent, messagedBy } = threadLineage("me", [], { id: "a", title: "Parent" });
    expect(parent).toMatchObject({ id: "a", title: "Parent", live: false, started: true });
    expect(parent?.link).toBeUndefined();
    expect(messagedBy).toBeUndefined();
  });

  test("only the parent's title survives pruning: the relation is still the parent", () => {
    const { parent, messagedBy } = threadLineage("me", [], { title: "The old parent" });
    expect(parent).toMatchObject({ title: "The old parent", live: false, started: true });
    expect(parent?.id).toBeUndefined();
    expect(messagedBy).toBeUndefined();
  });

  test("a later message from another thread is separate, not the parent", () => {
    const { parent, messagedBy } = threadLineage(
      "me",
      [
        link(1, { started: true, state: "done", createdAt: 10 }),
        link(2, {
          id: "b/1",
          orchestrator: "b",
          orchestratorTitle: "Bystander",
          state: "working",
          createdAt: 20,
        }),
      ],
      { id: "a", title: "Parent" },
    );
    expect(parent).toMatchObject({ id: "a", title: "Parent", live: false, started: true });
    expect(parent?.link?.id).toBe("a/1");
    expect(messagedBy).toMatchObject({ id: "b", title: "Bystander", live: true, started: false });
    expect(messagedBy?.link?.id).toBe("b/1");
  });

  test("a message from the parent is a follow-up, not a second relation", () => {
    const { parent, messagedBy } = threadLineage("me", [
      link(1, { started: true, state: "done", createdAt: 10 }),
      link(2, { started: false, state: "working", createdAt: 20 }),
    ]);
    expect(parent).toMatchObject({ id: "a", live: true, started: true });
    // The newest link from the same parent keeps the parent live; it is still one relation.
    expect(parent?.link?.id).toBe("a/2");
    expect(messagedBy).toBeUndefined();
  });

  test("a title-only parent never claims a same-titled messager", () => {
    const { parent, messagedBy } = threadLineage(
      "me",
      [
        link(1, {
          id: "b/1",
          orchestrator: "b",
          orchestratorTitle: "Parent",
          state: "working",
          createdAt: 10,
        }),
      ],
      { title: "Parent" },
    );
    // The title cannot prove the link belongs to the parent: the metadata stands alone.
    expect(parent).toMatchObject({ title: "Parent", live: false, started: true });
    expect(parent?.id).toBeUndefined();
    expect(parent?.link).toBeUndefined();
    // The link is still shown, as a messager, not mistaken for the parent.
    expect(messagedBy).toMatchObject({ id: "b", title: "Parent", live: true, started: false });
    expect(messagedBy?.link?.id).toBe("b/1");
  });

  test("a parent id matches only that id, not a same-titled thread", () => {
    const { parent, messagedBy } = threadLineage(
      "me",
      [
        link(1, {
          id: "b/1",
          orchestrator: "b",
          orchestratorTitle: "Parent",
          state: "done",
          createdAt: 10,
        }),
      ],
      { id: "a", title: "Parent" },
    );
    expect(parent).toMatchObject({ id: "a", title: "Parent", live: false });
    expect(parent?.link).toBeUndefined();
    expect(messagedBy).toMatchObject({ id: "b", title: "Parent", started: false });
  });

  test("a parent id and a link from that id are one relation", () => {
    const { parent, messagedBy } = threadLineage(
      "me",
      [
        link(1, {
          id: "a/1",
          orchestrator: "a",
          orchestratorTitle: "Renamed parent",
          state: "working",
          createdAt: 10,
        }),
      ],
      { id: "a", title: "Old name" },
    );
    expect(parent).toMatchObject({ id: "a", title: "Old name", live: true, started: true });
    expect(parent?.link?.id).toBe("a/1");
    expect(messagedBy).toBeUndefined();
  });

  test("a pruned parent and a live message from another thread show both", () => {
    const { parent, messagedBy } = threadLineage(
      "me",
      [
        link(2, {
          id: "b/1",
          orchestrator: "b",
          orchestratorTitle: "Bystander",
          state: "working",
          createdAt: 20,
        }),
      ],
      { id: "a", title: "Parent" },
    );
    expect(parent).toMatchObject({ id: "a", title: "Parent", live: false });
    expect(parent?.link).toBeUndefined();
    expect(messagedBy).toMatchObject({ id: "b", live: true });
  });

  test("a thread only messaged has a messager, not a parent", () => {
    const { parent, messagedBy } = threadLineage("me", [
      link(1, {
        id: "b/1",
        orchestrator: "b",
        orchestratorTitle: "Bystander",
        state: "done",
        createdAt: 10,
      }),
    ]);
    expect(parent).toBeUndefined();
    expect(messagedBy).toMatchObject({ id: "b", title: "Bystander", live: false, started: false });
  });

  test("metadata without a title still names a parent, with a neutral label", () => {
    const { parent } = threadLineage("me", [], { id: "a" });
    expect(parent).toMatchObject({ id: "a", title: "Another thread", live: false });
  });

  test("a live link keeps the relation live even if a newer one already settled", () => {
    const { parent } = threadLineage("me", [
      link(1, { started: true, state: "working", createdAt: 10 }),
      link(2, { started: false, state: "done", createdAt: 20 }),
    ]);
    expect(parent).toMatchObject({ live: true });
    expect(parent?.link?.id).toBe("a/1");
    const { messagedBy } = threadLineage("me", [
      link(1, {
        id: "b/1",
        orchestrator: "b",
        orchestratorTitle: "Bystander",
        state: "working",
        createdAt: 10,
      }),
      link(2, {
        id: "b/2",
        orchestrator: "b",
        orchestratorTitle: "Bystander",
        state: "done",
        createdAt: 20,
      }),
    ]);
    expect(messagedBy).toMatchObject({ id: "b", live: true });
    expect(messagedBy?.link?.id).toBe("b/1");
  });

  test("the newest link from a thread wins when older ones disagree", () => {
    const { parent } = threadLineage("me", [
      link(1, { started: true, orchestratorTitle: "Old name", createdAt: 10 }),
      link(2, { started: true, orchestratorTitle: "New name", createdAt: 20 }),
    ]);
    expect(parent).toMatchObject({ id: "a", title: "New name" });
    expect(parent?.link?.id).toBe("a/2");
  });

  test("the threads this one started and messaged are apart, and its parent is not one of them", () => {
    const { started, messaged } = threadLineage("me", [
      link(1, { id: "me/1", orchestrator: "me", target: "child", started: true, createdAt: 10 }),
      link(2, { id: "me/2", orchestrator: "me", target: "other", started: false, createdAt: 20 }),
      link(3, { id: "a/1", orchestrator: "a", target: "me", started: true, createdAt: 1 }),
    ]);
    expect(started.map((g) => g.link.target)).toEqual(["child"]);
    expect(messaged.map((g) => g.link.target)).toEqual(["other"]);
  });

  test("a follow-up to a thread it started is one row, kept by the started link", () => {
    const { started, messaged } = threadLineage("me", [
      link(1, { id: "me/1", orchestrator: "me", target: "child", started: true, createdAt: 10 }),
      link(2, { id: "me/2", orchestrator: "me", target: "child", state: "working", createdAt: 20 }),
    ]);
    // One thread, one row: the later message is not a second relation of the same thread.
    expect(started).toHaveLength(1);
    expect(messaged).toEqual([]);
    expect(started[0]?.links.map((l) => l.id)).toEqual(["me/1", "me/2"]);
    // The live follow-up speaks for the row, so it reads as the work it does now.
    expect(started[0]?.link.id).toBe("me/2");
  });

  test("two messages to the same thread are one row, not two", () => {
    const { started, messaged } = threadLineage("me", [
      link(1, { id: "me/1", orchestrator: "me", target: "other", createdAt: 10 }),
      link(2, { id: "me/2", orchestrator: "me", target: "other", createdAt: 20 }),
    ]);
    expect(started).toEqual([]);
    expect(messaged).toHaveLength(1);
    expect(messaged[0]?.link.id).toBe("me/2");
    expect(messaged[0]?.links.map((l) => l.id)).toEqual(["me/1", "me/2"]);
  });

  test("a broken record cannot make a thread its own parent", () => {
    const { parent, messagedBy, started, messaged } = threadLineage(
      "me",
      [link(1, { orchestrator: "me", target: "me", started: true, state: "working" })],
      { id: "me", title: "Me" },
    );
    expect(parent).toBeUndefined();
    expect(messagedBy).toBeUndefined();
    expect(started).toEqual([]);
    expect(messaged).toEqual([]);
  });

  test("the outbound list stays bounded like the details card always was", () => {
    const outbound = Array.from({ length: MAX_DELEGATES_PER_THREAD * 2 + 4 }, (_, i) =>
      link(i + 1, {
        id: `me/${i + 1}`,
        orchestrator: "me",
        target: `child-${i + 1}`,
        started: true,
        createdAt: i + 1,
      }),
    );
    const { started } = threadLineage("me", outbound);
    expect(started).toHaveLength(MAX_DELEGATES_PER_THREAD * 2);
    expect(started[0]?.link.target).toBe("child-5");
    expect(started.at(-1)?.link.target).toBe(`child-${MAX_DELEGATES_PER_THREAD * 2 + 4}`);
  });
});
