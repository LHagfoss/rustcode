# Security

RustCode runs shell commands and edits files on your machine, so security bugs
matter a lot here. Things like sandbox escapes, approval bypasses, leaked
credentials or problems in the `/remote` gateway.

## Reporting

Please don't open a public issue for a security problem.

Report it privately through
[GitHub's private vulnerability reporting](https://github.com/LHagfoss/rustcode/security/advisories/new),
or email [hello@lhagfoss.com](mailto:hello@lhagfoss.com).

Include what you found, how to reproduce it, and the RustCode version
(`rustcode --version`). I'll get back to you as soon as I can and keep you
posted on the fix.

## Supported versions

Only the latest release gets fixes. Update with `rustcode --update`.
