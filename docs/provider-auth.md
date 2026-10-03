# Provider authentication

RustCode separates a model profile from its authentication. Profiles describe a model endpoint and protocol; provider account credentials are stored by the platform credential manager and referenced from profiles. Existing `[[models]]` entries that use `api_key` or `env_key` remain supported.

## Commands

- `/login` lists configured providers and the authentication methods each supports.
- `/login <provider>` starts that provider's default sign-in method when configured. `/login openai` starts ChatGPT sign-in, reusing the selected or sole saved account's registration when possible. If multiple accounts exist and none is selected, choose one from `/auth status`; `/login openai <account-id>` reconnects that account, and `/login openai new` adds another account.
- `/login <provider> api-key <ENV_VAR>` reads the named environment variable and stores the API key in the operating system credential store. Pass the variable name, never the key itself. RustCode fails if the native credential store is unavailable; it does not write a plaintext fallback.
- `/auth status` lists saved provider accounts, methods, state, and account IDs. Use the account ID to target one of several accounts.
- `/logout <provider> [account-id]` revokes or removes the selected saved connection. Omitting the ID signs out all matching accounts.

Logout prevents later requests from resolving the saved credential. A request that already resolved its access token may finish; logout cannot retract an inference already sent to a provider.

Authentication commands run outside the model prompt, do not appear in prompt history or arrow-key recall, and show progress while a browser sign-in is pending.

## OpenAI ChatGPT plan

`/login openai` opens OpenAI's Sign in with ChatGPT flow. Eligible Plus and Pro users can authorize RustCode to make supported Responses API requests using their ChatGPT plan. This is distinct from `OPENAI_API_KEY`, which uses OpenAI Platform API billing. The open-source/local-app flow uses OAuth and does not need an API key or client secret. OpenAI currently documents ChatGPT plan usage for open-source and locally hosted apps; paid or remotely hosted apps should consult OpenAI's eligibility process.

The login saves the account connection and creates/selects a ChatGPT Responses profile only after sign-in succeeds. Select that profile with `/model`. The first model returned by the account's catalog is selected automatically; catalog availability and capabilities can vary, so choose a vision-capable model when needed. RustCode sends image input through the supported Responses request path, but a model that does not support vision will still reject image prompts. ChatGPT plan usage follows OpenAI's current Responses API preview restrictions; it is not a drop-in compatibility mode for every Responses API parameter or hosted tool.

When a ChatGPT plan profile is selected, `/compact` uses RustCode's deterministic local history compaction. It does not send a separate non-streaming compaction request to OpenAI.

RustCode writes the returned profile to the user config and keeps the account secret in the OS credential store. The equivalent profile binding looks like this; normally `/login` creates it for you:

```toml
[[models]]
name = "openai-chatgpt-gpt-5-codex-ACCOUNTID"
url = "https://api.openai.com/v1/responses"
model = "gpt-5-codex"
api_protocol = "responses"

[models.credential]
provider = "openai"
account = "ACCOUNT_ID_FROM_AUTH_STATUS"
method = "chat_gpt"
```

Use the exact account ID printed by `/auth status` in `account`; the example model and profile name are illustrative. A profile credential reference contains no token itself. Keep a credential-bound profile pointed at its configured provider endpoint. To reconnect a saved ChatGPT account, use `/login openai <account-id>`; to add another account, use `/login openai new`; to remove an account, use `/logout openai <account-id>`.

See [OpenAI's Sign in with ChatGPT overview](https://developers.openai.com/siwc/token-sharing-open-source), [registration and sign-in](https://developers.openai.com/siwc/token-sharing-open-source/sign-in), and [models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) for current scope, token, and request requirements.

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

When a credential is attached to a profile, keep that profile's provider endpoint. Do not copy an account credential onto a profile pointing at an unrelated host. Legacy profiles without a credential reference continue using their existing API-key or environment-variable settings.
