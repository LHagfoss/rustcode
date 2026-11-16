// "Create with agent" (Settings › Capabilities, the command palette): a new thread whose draft
// asks an agent to write a skill, closing Settings so the reader lands in the composer.

import { beforeEach, expect, test } from "bun:test";
import { resetForTests, useApp } from "./store";
import { LOCAL_HOST } from "./threadKeys";
import { CREATE_SKILL_PROMPT, createSkillWithAgent } from "./threadOpen";

beforeEach(() => resetForTests());

test("starts a new thread on this computer with the skill draft and closes Settings", () => {
  useApp.setState({ settings: "capabilities" });
  createSkillWithAgent();
  const s = useApp.getState();
  expect(s.settings).toBeNull();
  expect(s.selectedId).toBeNull();
  expect(s.newThreadHost).toBe(LOCAL_HOST);
  expect(s.drafts.new).toBe(CREATE_SKILL_PROMPT);
});

test("the draft names both ways to save a skill and ends open for the request", () => {
  expect(CREATE_SKILL_PROMPT).toContain("create-skill");
  expect(CREATE_SKILL_PROMPT).toContain('action "create"');
  expect(CREATE_SKILL_PROMPT).toContain("~/.config/rustcode/skills/<name>/SKILL.md");
  expect(CREATE_SKILL_PROMPT.endsWith("The skill should: ")).toBe(true);
});
