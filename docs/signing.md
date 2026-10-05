# Release signing (maintainers)

Windows releases are built by GitHub Actions and, once SignPath is configured, signed through
[SignPath Foundation](https://signpath.org/) before they are published. Until then the release
contains unsigned files. See [CODE_SIGNING.md](../CODE_SIGNING.md) for the public policy. Linux
binaries are not signed; the release lists their SHA-256 checksums.

## One-time setup

1. Enable two-factor authentication on GitHub (required by SignPath for every team member).
2. Apply at <https://signpath.org/apply> with the repository URL.
3. Once approved, in SignPath:
   - Create the project with the slug `coolercast` and link the GitHub repository as a trusted
     build system.
   - Add the two artifact configurations below, with the slugs `executables` and `installers`.
   - Create a signing policy with the slug `release-signing`, using the SignPath Foundation
     certificate and manual approval.
   - Create a CI user, give it submitter rights on the policy and copy its API token.
4. In the GitHub repository settings:
   - Secret `SIGNPATH_API_TOKEN`: the CI user's API token.
   - Variable `SIGNPATH_ORGANIZATION_ID`: the SignPath organization ID.

Setting the variable is what turns signing on in [ci.yml](../.github/workflows/ci.yml) for tags.

### Artifact configurations

The `windows` job signs in two rounds, because the installers must contain signed executables:

1. `executables`: the `x64` and `arm64` folders with `coolercast.exe` and `coolercast-app.exe`.
2. `installers`: the MSI files built from the signed executables.

Product name and version must match the metadata embedded by `assets/windows/resources.rs` and
`packaging/windows/coolercast.wxs`.

```xml
<?xml version="1.0" encoding="utf-8"?>
<!-- slug: executables -->
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <parameters>
    <parameter name="version" default-value="0.0.0" />
  </parameters>
  <zip-file>
    <directory path="x64">
      <pe-file path="coolercast.exe" product-name="CoolerCast" product-version="${version}">
        <authenticode-sign />
      </pe-file>
      <pe-file path="coolercast-app.exe" product-name="CoolerCast" product-version="${version}">
        <authenticode-sign />
      </pe-file>
    </directory>
    <directory path="arm64">
      <pe-file path="coolercast.exe" product-name="CoolerCast" product-version="${version}">
        <authenticode-sign />
      </pe-file>
      <pe-file path="coolercast-app.exe" product-name="CoolerCast" product-version="${version}">
        <authenticode-sign />
      </pe-file>
    </directory>
  </zip-file>
</artifact-configuration>
```

```xml
<?xml version="1.0" encoding="utf-8"?>
<!-- slug: installers -->
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <parameters>
    <parameter name="version" default-value="0.0.0" />
  </parameters>
  <zip-file>
    <msi-file path="coolercast-${version}-windows-x64.msi">
      <authenticode-sign />
    </msi-file>
    <msi-file path="coolercast-${version}-windows-arm64.msi">
      <authenticode-sign />
    </msi-file>
  </zip-file>
</artifact-configuration>
```

## Releasing

1. Bump `version` in the workspace `Cargo.toml` and commit.
2. Tag and push: `git tag -a vX.Y.Z -m "CoolerCast X.Y.Z"` and `git push origin vX.Y.Z`.
3. The `windows` job waits for both signing requests to be approved in SignPath. The `release`
   job then publishes the MSI installers, the portable zips, the Linux tarballs and
   `SHA256SUMS.txt`. The build fails if the tag does not match the crate version.
