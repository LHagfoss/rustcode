# Agent verification TODO & recipes

Below is a set of test tasks designed to verify that your agent can use the new tools (`create_file`, `write_file`, `delete_file`, `move_file`, and `copy_file`) and that the confirmation popup handles inputs correctly.

---

## How to run the verification:
1. Start your `rustcode` TUI client.
2. Ensure you have the `/tmp/rustcode-debug.log` running in another terminal pane to inspect logs if needed:
   ```bash
   tail -f /tmp/rustcode-debug.log
   ```
3. Copy and paste the prompts below into the TUI chat input.

---

### Task 1: Create a New File (Tests `create_file` & Confirmation Gate)
* **Prompt**:
  > Create a file at `/tmp/agent_test.txt` containing the text: "Agent verification line 1\nAgent verification line 2"
* **Expected Behavior**:
  1. The agent should parse the task and emit a `<tool_call>` for `create_file`.
  2. The TUI will show a **modal popup** asking for approval.
  3. The popup header should show `⚠ Create file?` and the body should show `/tmp/agent_test.txt`, size `52 bytes`, and a preview of the content.
  4. Press `Enter` (or `y`/`Y`) to approve.
  5. The agent should receive the success result and answer you.

---

### Task 2: Modify the File (Tests `write_file` & Auto-Confirm Toggle)
* **Prompt**:
  > Overwrite the file at `/tmp/agent_test.txt` to add a third line: "Agent verification line 3"
* **Expected Behavior**:
  1. The agent should emit a `<tool_call>` for `write_file`.
  2. The TUI will pop up the tool confirmation modal.
  3. **Press `Tab`** to toggle auto-confirm. You should see `[x] Auto-confirm future tool calls` appear on the toggle line.
  4. Press `Enter` (or `y`/`Y`) to approve.
  5. The file is modified. Any future confirmation prompts will be approved automatically without showing the modal.

---

### Task 3: Copy and Move (Tests `copy_file` & `move_file`)
* **Prompt**:
  > Copy `/tmp/agent_test.txt` to `/tmp/agent_test_copy.txt`, then move `/tmp/agent_test.txt` to `/tmp/agent_test_moved.txt`
* **Expected Behavior**:
  1. The agent will call `copy_file` followed by `move_file`.
  2. Because auto-confirm is enabled, **no modal popup will appear**; both tools will execute immediately.
  3. Verify that `/tmp/agent_test_copy.txt` and `/tmp/agent_test_moved.txt` both exist, and the original `/tmp/agent_test.txt` is gone.

---

### Task 4: Cleanup (Tests `delete_file`)
* **Prompt**:
  > Delete both `/tmp/agent_test_copy.txt` and `/tmp/agent_test_moved.txt`
* **Expected Behavior**:
  1. The agent will delete both files.
  2. With auto-confirm still active, it will proceed silently.
  3. Verify the files are deleted successfully.
