// Shared by the build scripts of the executables (`include!`d): generates and embeds a Windows
// resource script with the icon, the manifest and the version information. The version
// metadata is required for code signing.

fn embed_windows_resources(description: &str, file_name: &str) {
    use std::path::{Path, PathBuf};
    use std::{env, fs};

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = manifest_dir.join("../../assets");
    let icon = assets.join("coolercast.ico");
    let manifest = assets.join("windows/app.manifest");
    for path in [&icon, &manifest] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=../../assets/windows/resources.rs");

    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (numeric[0], numeric[1], numeric[2]);
    let rc_path = |path: &Path| path.display().to_string().replace('\\', "\\\\");

    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "CoolerCast"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{file_name}"
      VALUE "LegalCopyright", "Copyright (c) 2026 Moisés Casanova. MIT License."
      VALUE "OriginalFilename", "{file_name}"
      VALUE "ProductName", "CoolerCast"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = rc_path(&icon),
        manifest = rc_path(&manifest),
    );

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("resources.rc");
    fs::write(&out, rc).unwrap();
    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
