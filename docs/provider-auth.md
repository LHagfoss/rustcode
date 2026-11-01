# Provider authentication

RustCode separates a model profile from its authentication. Profiles describe a model endpoint and protocol; provider account credentials are stored by the platform credential manager and referenced from profiles. Existing `[[models]]` entries that use `api_key` or `env_key` remain supported.

## Commands

- `/login` lists configured providers and the authentication methods each supports.
- `/login <provider>` starts that provider's default sign-in method when configured. `/login openai` refreshes the selected or sole saved ChatGPT model catalog without opening a browser; if no account is saved, it starts ChatGPT sign-in. If multiple accounts exist and none is selected, choose one from `/auth status`; `/login openai <account-id>` reconnects that account, and `/login openai new` adds another account.
- `/login openai` and `/login github-copilot` take `--browser` or `--headless` anywhere after the provider. `--headless` never opens a browser: it shows the URL (and for GitHub the device code) to open on any device. `--browser` opens the browser on this machine and fails if it cannot. With neither, the browser is opened unless the session is reached over SSH or is a Linux session without a display, and the URL is shown when the browser cannot be opened. ChatGPT sign-in finishes on `http://127.0.0.1:<port>` on the machine running RustCode; when the browser is on another machine, forward that port first with the `ssh -L` command shown next to the URL. The shown URL never carries a saved identity token.
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

## Anthropic API

Claude subscription sign-in through the local Claude Code CLI is currently unavailable in RustCode. Anthropic API access remains supported using an API key, billed separately from Claude plans. Set `ANTHROPIC_API_KEY` in the environment RustCode reads. After adding a profile as shown below, run `/login anthropic`; RustCode stores the key in the OS credential store, not in the config file.

Add a model profile for the Anthropic Messages API and select it with `/model`:

```toml
[[models]]
name = "anthropic/sonnet"
url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-5"
api_protocol = "anthropic_messages"
```

Add the profile before running `/login anthropic`. Login binds the active matching profile, or the sole matching profile if there is only one. If no profile can be selected, it saves the key and prints the credential reference to add manually. Use Anthropic's current model catalog for a model ID available to your account. Existing `claude_cli` profiles are retained when loading old config, but requests and catalog refreshes for those profiles now fail with a message explaining that this sign-in method is unsupported.

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

On macOS, credentials are stored in `~/.config/rustcode/credentials.json`, readable only by your user (mode `0600`). On other platforms they are stored in the native credential manager (for example, a Linux Secret Service keyring). On headless Linux, ensure a Secret Service provider is installed, available to the user session, and unlocked; if it cannot be opened, `/login` fails and leaves the key out of RustCode's config. Set `RUSTCODE_CREDENTIAL_STORE=file` to use the file on any platform, or `RUSTCODE_CREDENTIAL_STORE=keychain` to keep using the native store.

### Why macOS does not use Keychain

Keychain grants access to one signed identity. RustCode is not signed with an Apple team ID, so after each update macOS treats it as a new program and asks for the login password again for every stored credential, even after "Always Allow". The file never asks.

Credentials saved in Keychain by an earlier version move to the file the first time each one is read; that read can ask one last time. The Keychain items are left in place; delete entries for `org.rustcode.provider-auth` in Keychain Access once you no longer need them.

When a credential is attached to a profile, keep that profile's provider endpoint. Do not copy an account credential onto a profile pointing at an unrelated host. Legacy profiles without a credential reference continue using their existing API-key or environment-variable settings.
