# Security policy

Security fixes are supported for the latest release on each maintained major
version.

Please do not report vulnerabilities in public issues. Use GitHub's private
vulnerability reporting page:

https://github.com/danielkov/dioxus-editor/security/advisories/new

Include affected versions, impact, reproduction steps, and any known
mitigation. Maintainers will acknowledge a report as soon as practical and
coordinate disclosure after a fix is available.

Application-supplied decorator renderers are a trust boundary. Decorator
attributes may come from untrusted markdown or stored content, so renderers
must validate URLs and any other security-sensitive values before emitting
DOM attributes.
