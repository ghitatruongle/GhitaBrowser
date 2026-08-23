# Security Policy

## Supported versions

Only the latest 2.x release receives security fixes.

## Reporting a vulnerability

Do not open a public issue containing exploit details or user data. Report the
problem privately to the repository maintainers through GitHub's private
security advisory feature. Include affected versions, reproduction steps,
impact and any suggested mitigation.

Maintainers should acknowledge a report within seven days, keep the reporter
informed, and publish a coordinated fix and advisory when practical.

## Security boundaries

GhitaBrowser 2.0 is a document-focused browser. Its built-in JavaScript engine
does not provide a complete DOM or the isolation guarantees of a mature
multi-process browser. Do not use it for high-risk authentication, payments or
untrusted active web applications.

Untrusted HTML and PDF preparation runs in a short-lived renderer worker with
bounded request/response frames, checked decompression, parser limits and a
15-second timeout. This crash boundary is not a Windows AppContainer or
low-integrity sandbox, so it reduces shell crashes but must not be described as
complete operating-system sandboxing.

Local subresources are restricted to the selected document's directory. The
PDF reader rejects encrypted files and enforces byte, object, page, stream and
text limits. It does not execute embedded PDF actions or JavaScript.

The legacy password-store module uses reversible obfuscation and is not a
supported password manager. It is excluded from the 2.0 user interface until
an operating-system credential vault is used.

## Dependency status

The 2.0.0 locked dependency graph had no known RustSec vulnerability at the time
it was audited, but it retained informational warnings in the transitive Iced
0.12 graphics and font stack. The public-release security audit remains
available through the full release gate; it is optional for a personal build,
and this statement does not claim a newer audit result.
