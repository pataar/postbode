# Security policy

Postbode holds IMAP credentials and can delete mail, so security reports are welcome.

## Reporting a vulnerability

Report it privately through [GitHub's private vulnerability reporting](https://github.com/pataar/postbode/security/advisories/new). Do not open a public issue.

Include the version (`postbode --version`), your platform, and the steps to reproduce. Leave out real passwords, tokens and message contents.

You can expect a first reply within a week. Once a fix is released, the advisory is published with credit to you, unless you would rather stay anonymous.

## Supported versions

Only the latest release gets security fixes.

## In scope

- Leaking credentials, for example through logs, error messages or files with loose permissions
- A rule or command deleting or moving mail it should not touch
- Skipping the `.eml` backup before a rule deletes mail
- TLS verification problems when talking to the IMAP server
- Crafted messages that crash Postbode or corrupt the local store
