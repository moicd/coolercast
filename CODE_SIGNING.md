# Code signing policy

> **Status:** the application to the SignPath Foundation is pending. Until it is approved, release
> binaries are not signed.

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/).

## What is signed

Only `coolercast.exe` and `coolercast-app.exe` are signed, and only when they are built by
[GitHub Actions](.github/workflows/ci.yml) from a release tag of this repository. Every signing
request is approved manually. The PawnIO modules shipped in the release (`modules\*.bin`) are
third-party files and are not signed by this project.

## Team roles

| Role | Members |
|---|---|
| Committers and reviewers | [moicd](https://github.com/moicd) |
| Approvers | [moicd](https://github.com/moicd) |

## Privacy policy

This program will not transfer any information to other networked systems unless specifically
requested by the user or the person installing or operating it. CoolerCast does not use the network
at all: it talks to the cooler over USB and to its own service over a local named pipe.
