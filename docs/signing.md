# Release signing (maintainers)

Releases are built by GitHub Actions and, once SignPath is configured, signed through
[SignPath Foundation](https://signpath.org/) before they are published. Until then the release job
publishes the unsigned build. See [CODE_SIGNING.md](../CODE_SIGNING.md) for the public policy.

## One-time setup

1. Enable two-factor authentication on GitHub (required by SignPath for every team member).
2. Apply at <https://signpath.org/apply> with the repository URL.
3. Once approved, in SignPath:
   - Create the project with the slug `coolercast` and link the GitHub repository as a trusted
     build system.
   - Add the artifact configuration below as the project's default configuration.
   - Create a signing policy with the slug `release-signing`, using the SignPath Foundation
     certificate and manual approval.
   - Create a CI user, give it submitter rights on the policy and copy its API token.
4. In the GitHub repository settings:
   - Secret `SIGNPATH_API_TOKEN`: the CI user's API token.
   - Variable `SIGNPATH_ORGANIZATION_ID`: the SignPath organization ID.

Setting the variable is what turns signing on in [ci.yml](../.github/workflows/ci.yml).

### Artifact configuration

The build uploads the release folder as a GitHub artifact; SignPath receives it as a zip file.
Product name and version must match the metadata embedded by `assets/windows/resources.rs`.

```xml
<?xml version="1.0" encoding="utf-8"?>
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <parameters>
    <parameter name="version" default-value="0.0.0" />
  </parameters>
  <zip-file>
    <pe-file path="coolercast.exe" product-name="CoolerCast" product-version="${version}">
      <authenticode-sign />
    </pe-file>
    <pe-file path="coolercast-app.exe" product-name="CoolerCast" product-version="${version}">
      <authenticode-sign />
    </pe-file>
  </zip-file>
</artifact-configuration>
```

## Releasing

1. Bump `version` in the workspace `Cargo.toml` and commit.
2. Tag and push: `git tag -a vX.Y.Z -m "CoolerCast X.Y.Z"` and `git push origin vX.Y.Z`.
3. The `release` job waits for the signing request to be approved in SignPath, then publishes the
   GitHub release with the signed zip. The job fails if the tag does not match the crate version.
