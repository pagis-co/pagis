# Security policy

## Supported versions

Only the latest release of Pagis gets security fixes. To get a fix,
update to the latest release.

| Version | Supported |
| --- | --- |
| The Client App, Server Package and Headless Server image of the latest release | Yes |
| The Computer Image version that the latest release pins | Yes |
| Each earlier release, and each other Computer Image version | No |

The Client App, the Server Package and the Headless Server image of a
release carry one version. Thus the latest release covers each
installation method. The Computer Image has a version of its own, and
each release pins one Computer Image version.

## Reporting a vulnerability

Use GitHub private vulnerability reporting. It is the only route for a
vulnerability report. It keeps the report private between you and the
maintainers until they publish a security advisory.

1. On the GitHub page of this repository, open the security tab.
2. Click **Report a vulnerability**.
3. Complete the form and submit it.

Do not open a public issue, discussion or pull request for a
vulnerability. A public report discloses the defect before a fix is
available.

Put this information in the report:

- The affected version: the release, and the Computer Image version when
  the defect is in a Computer.
- The installation method: the Headless Server, a Local Installation
  with Multi-User Mode on or off, or a Client App connected to a Server.
- The steps to reproduce the defect.
- The impact: what an attacker can read, change or run.

The maintainers keep these response targets:

- They acknowledge the report in 3 business days.
- They send a first assessment in 7 days.
- They disclose the vulnerability in coordination with you, not more
  than 90 days after the report.

The maintainers publish each fix as a GitHub security advisory. The
advisory gives credit to the reporter.

## Scope

This policy covers the code in this repository and the release artifacts
that are built from it:

- the Client App;
- the Server Package;
- the Headless Server image;
- the Computer Image;
- the deployment files in `deploy/`.

These are out of scope:

- third-party services, for example a model provider;
- the proxy or tunnel that an operator puts in front of an installation.
