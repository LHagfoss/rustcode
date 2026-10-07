# Provider authentication

RustCode separates a model profile from its authentication. Profiles describe a model endpoint and protocol; provider account credentials are stored by the platform credential manager and referenced from profiles. Existing `[[models]]` entries that use `api_key` or `env_key` remain supported.

## Commands

- `/login` lists configured providers and the authentication methods each supports.
- `/login <provider>` starts that provider's default sign-in method when configured. `/login openai` refreshes the selected or sole saved ChatGPT model catalog without opening a browser; if no account is saved, it starts ChatGPT sign-in. If multiple accounts exist and none is selected, choose one from `/auth status`; `/login openai <account-id>` reconnects that account, and `/login openai new` adds another account.
- `/login claude` connects the account signed in to the local Claude Code CLI and installs its models; see [Claude (Claude Code CLI)](#claude-claude-code-cli).
- `/login <provider> api-key <ENV_VAR>` reads the named environment variable and stores the API key in the operating system credential store. Pass the variable name, never the key itself. RustCode fails if the native credential store is unavailable; it does not write a plaintext fallback.
- `/auth status` lists saved provider accounts, methods, state, and account IDs. Use the account ID to target one of several accounts.
- `/accounts` lists saved provider accounts. `/account` shows the active model profile and its provider account status.
- `/refresh [provider] [account-id]` refreshes the selected account's model catalog without opening a browser; omit arguments to refresh the active account. `/account refresh` is the same command.
- `/logout <provider> [account-id]` revokes or removes the selected saved connection. Omitting the ID signs out all matching accounts.
- `/status` includes the active provider account with session details. `/usage` keeps local token totals and adds provider usage guidance; those local totals are not billing or quota data.

Logout prevents later requests from resolving the saved credential. A request that already resolved its access token may finish; logout cannot retract an inference already sent to a provider.

Authentication commands run outside the model prompt and never enter provider conversation history. Safe, well-formed auth commands appear in arrow-key recall; malformed commands and commands containing a literal key are excluded. Environment-variable arguments are recalled only when they match the expected variable-name format.

## OpenAI ChatGPT plan

On first sign-in, `/login openai` opens OpenAI's Sign in with ChatGPT flow. With a saved account, it refreshes the selected or sole account's model catalog without opening a browser. Eligible Plus and Pro users can authorize RustCode to make supported Responses API requests using their ChatGPT plan. This is distinct from `OPENAI_API_KEY`, which uses OpenAI Platform API billing. The open-source/local-app flow uses OAuth and does not need an API key or client secret. OpenAI currently documents ChatGPT plan usage for open-source and locally hosted apps; paid or remotely hosted apps should consult OpenAI's eligibility process.

The first sign-in saves the account connection and creates profiles for all currently listed models only after sign-in succeeds. It selects one available model profile; catalog availability and capabilities can vary, so choose a vision-capable model with `/model` when needed. RustCode sends image input through the supported Responses request path, but a model that does not support vision will still reject image prompts. `/login openai` refreshes the saved account's model catalog and installs newly listed models without repeating browser sign-in. `/account refresh [provider] [account-id]` refreshes a chosen saved account's catalog. ChatGPT plan usage follows OpenAI's current Responses API preview restrictions; it is not a drop-in compatibility mode for every Responses API parameter or hosted tool.

When a ChatGPT plan profile is selected, `/compact` uses RustCode's deterministic local history compaction. It does not send a separate non-streaming compaction request to OpenAI.

For a ChatGPT plan profile, `/usage` shows a bar per subscription limit window (used share and reset time) once OpenAI reports those limits with a response; until then it links to [ChatGPT Settings → Usage](https://chatgpt.com/settings/usage). With an `OPENAI_API_KEY` profile, API usage and billing are managed separately in the [OpenAI API platform](https://platform.openai.com/usage). The `/usage` panel also reports RustCode's local prompt/completion token totals and monthly session totals, which do not measure provider billing or ChatGPT plan limits.

RustCode writes the returned profile to the user config and keeps the account secret in the OS credential store. The equivalent profile binding looks like this; normally `/login` creates it for you:

```toml
[[models]]
name = "chatgpt/gpt-6-astra"
url = "https://api.openai.com/v1/responses"
model = "gpt-6-astra"
api_protocol = "responses"

[models.credential]
provider = "openai"
account = "ACCOUNT_ID_FROM_AUTH_STATUS"
method = "chat_gpt"
```

Use the exact account ID printed by `/auth status` in `account`; the example model and profile name are illustrative and may not appear in every account's catalog. A profile credential reference contains no token itself. Keep a credential-bound profile pointed at its configured provider endpoint. To reconnect a saved ChatGPT account, use `/login openai <account-id>`; to add another account, use `/login openai new`; to remove an account, use `/logout openai <account-id>`.

See [OpenAI's Sign in with ChatGPT overview](https://developers.openai.com/siwc/token-sharing-open-source), [registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), and [models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) for current scope, token, and request requirements.

## GitHub Copilot

`/login github-copilot` connects with the installed GitHub CLI credential or, when `RUSTCODE_COPILOT_CLIENT_ID` names your own OAuth app, that app's device flow. It installs `copilot/<id>` profiles from the authenticated model catalog; `/refresh github-copilot` re-fetches them. See [GitHub Copilot setup](github-copilot.md) for prerequisites, commands, model rules, and limitations.

## Claude (Claude Code CLI)

`/login claude` connects RustCode to the account signed in to the locally installed Claude Code CLI (`claude`). It installs one `claude/<model>` profile per model the CLI offers; pick one with `/model`. `/refresh claude` re-reads the list.

Prerequisites: install the CLI and sign in with `claude auth login`. If the binary is not on `PATH`, set `RUSTCODE_CLAUDE_CLI` to its location.

How it works:

- RustCode never reads, copies, or stores the CLI's credential. A profile's `url` is the marker `claude-cli://local`; requests go to a headless `claude` child process, not to an HTTP endpoint, and use that CLI's sign-in and plan limits. `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are removed from the child's environment so it cannot silently bill an API key instead.
- The child runs with none of its own tools, settings, hooks, MCP servers, slash commands, or saved sessions. RustCode's tools are offered to it over an in-process channel, so every tool call is executed and authorized by RustCode exactly as with other providers, and RustCode's system prompt replaces the CLI's.
- The child keeps the conversation's context while it lives. When RustCode's history changes in a way the child has not seen (rewind, `/compact`, resuming a saved session, switching model or reasoning effort, newly available tools, a cancelled response, or 30 idle minutes), RustCode starts a new child and restores the earlier turns as a text transcript. That restore is lossy: earlier tool calls become prose.
- `/usage` shows the five-hour and seven-day plan windows once the CLI reports them with a response. `/compact` uses RustCode's deterministic local compaction for these profiles.
- `/logout claude` disconnects RustCode and stops its child processes; the CLI itself stays signed in until you run `claude auth logout`.

Limitations: the reasoning budget and output-token settings of a profile are not applied (the CLI manages them; `reasoning_effort` is passed when it is one of `low`, `medium`, `high`, `xhigh`, `max`), a system prompt change takes effect only when a new child starts, and model reasoning is shown only when the CLI streams it. Check Anthropic's current terms for using a Claude subscription through the CLI from other software before relying on this for anything beyond your own use.

```toml
[[models]]
name = "claude/sonnet"
url = "claude-cli://local"
model = "sonnet"
api_protocol = "anthropic_messages"

[models.credential]
provider = "claude"
account = "ACCOUNT_ID_FROM_AUTH_STATUS"
method = "claude_cli"
```

## Add an API-key provider

Provider definitions can be added to the configuration without changing existing model profiles:

```toml
[[providers]]
id = "my-provider"
display_name = "My Provider"
api_key_env = "MY_PROVIDER_API_KEY"
auth_methods = ["api_key"]
base_url = "https://api.example.com/v1"
```

Then export the key and run `/login my-provider api-key MY_PROVIDER_API_KEY`. Provider IDs are stable configuration identifiers; display names are for menus. A custom provider entry advertises only the auth methods configured for it. Adding a provider does not automatically enable OAuth or subscription-based login: those flows require an explicit provider integration and its supported request protocol.

Credentials are stored in the platform's native credential manager (for example, macOS Keychain or a Linux Secret Service keyring). On headless Linux, ensure a Secret Service provider is installed, available to the user session, and unlocked. If the native store cannot be opened, `/login` fails and leaves the key out of RustCode's config; there is no plaintext fallback.

### macOS asks for the login password after every update

Keychain grants access to one signed identity. A release binary, and one built with `cargo install`, is ad-hoc signed: its identity is the hash of that exact build, so each update is a new program to Keychain and it asks again for every stored credential, even after "Always Allow".

Run `scripts/macos-stable-signature.sh` once. It creates a self-signed certificate named "RustCode Local Signing" in your login keychain and signs the installed binary with it. `rustcode update` and `scripts/install.sh` sign with the same certificate afterwards, so the identity no longer changes. The first run after signing asks one last time per credential; choose "Always Allow". After `cargo install`, run the script again, because Cargo replaces the signature.

RustCode also reads each stored credential once per process rather than on every request.

When a credential is attached to a profile, keep that profile's provider endpoint. Do not copy an account credential onto a profile pointing at an unrelated host. Legacy profiles without a credential reference continue using their existing API-key or environment-variable settings.
