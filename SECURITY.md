# Security policy

Only the latest published prerelease is supported. Hive is early software;
reports are handled on a best-effort basis without a response-time guarantee.

Please use [Report a vulnerability](https://github.com/vitaliits-stack/hive/security/advisories/new)
for security issues. Include the affected version, reproduction steps, impact,
and a minimal example with synthetic data. Do not post credentials, private source,
personal information, or vulnerability details in public issues.

If the private reporting form is unavailable, open an issue asking the maintainer
to enable private reporting, without disclosing the vulnerability.

Hive launches local shells and developer tools with your user permissions; it is
not a sandbox for untrusted repositories or agents. Review destructive Git actions
and generated commit messages. AI generation sends staged source to the provider
you select. Usage integrations read existing CLI credentials.

The initial macOS release is ad-hoc signed and not notarized. Download only from
this repository's Releases page. Checksums detect accidental file changes but are
not a replacement for a trusted signing identity. Review and redact logs before sharing.
