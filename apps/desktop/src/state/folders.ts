// ⌘O and ⌘⇧O: the folder a thread works in. ⌘O picks the folder of the
// thread in front: a new one starts there, an open one moves there
// (`moveThread`). ⌘⇧O picks where new threads start by default.

import { pickFolder } from "../lib/native";
import { moveThread, setDefaultCwd, setProjectCwd, toast, useApp } from "./store";

export async function chooseFolder(asDefault: boolean): Promise<void> {
  const s = useApp.getState();
  if (asDefault) {
    const picked = await pickFolder(s.defaultCwd || s.projectCwd);
    if (picked) {
      setDefaultCwd(picked);
    }
    return;
  }
  const id = s.selectedId;
  // A running thread cannot move: say so before the picker, not after it.
  if (id && s.threads[id]?.running) {
    toast("Stop the running turn before moving this thread to another folder.");
    return;
  }
  const picked = await pickFolder((id ? s.threads[id]?.cwd : undefined) ?? s.projectCwd);
  if (!picked) return;
  if (id) void moveThread(id, picked);
  else setProjectCwd(picked);
}
