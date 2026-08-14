use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const ASSETS: &[(&str, &[u8])] = &[
    (
        "extension/index.ts",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/extension/index.ts"
        )),
    ),
    (
        "extension/protocol.ts",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/extension/protocol.ts"
        )),
    ),
    (
        "extension/tool-client.ts",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/extension/tool-client.ts"
        )),
    ),
    (
        "extension/tools/read.ts",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/extension/tools/read.ts"
        )),
    ),
    (
        "extension/tools/terminal.ts",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/extension/tools/terminal.ts"
        )),
    ),
    (
        "tool-contract-v2.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/pi/tool-contract-v2.json"
        )),
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedAssets {
    pub root: PathBuf,
    pub extension: PathBuf,
    pub digest: String,
}

#[derive(Debug)]
pub struct AssetError {
    path: PathBuf,
    source: std::io::Error,
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot materialize Pi runtime asset {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub fn materialize(cache_root: &Path) -> Result<MaterializedAssets, AssetError> {
    let digest = asset_digest();
    let runtime_root = cache_root.join("pi-runtime");
    let root = runtime_root.join(&digest);
    fs::create_dir_all(&root).map_err(|source| AssetError {
        path: root.clone(),
        source,
    })?;
    for (relative, content) in ASSETS {
        write_asset(&root, relative, content)?;
    }
    Ok(MaterializedAssets {
        extension: root.join("extension/index.ts"),
        root,
        digest,
    })
}

fn asset_digest() -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, content) in ASSETS {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        hasher.update(content);
    }
    hasher.finalize().to_hex().to_string()
}

fn write_asset(root: &Path, relative: &str, content: &[u8]) -> Result<(), AssetError> {
    let path = root.join(relative);
    let parent = path.parent().expect("asset paths always have parents");
    fs::create_dir_all(parent).map_err(|source| AssetError {
        path: parent.to_path_buf(),
        source,
    })?;
    if let Ok(existing) = fs::read(&path)
        && existing == content
    {
        return Ok(());
    }
    fs::write(&path, content).map_err(|source| AssetError { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_asset_contents(root: &Path) {
        for (relative, content) in ASSETS {
            assert_eq!(
                fs::read(root.join(relative)).unwrap(),
                *content,
                "asset content differs for {relative}"
            );
        }
    }

    #[test]
    fn materializes_and_repairs_all_embedded_assets() {
        let directory = tempfile::tempdir().unwrap();
        let first = materialize(directory.path()).unwrap();
        assert_asset_contents(&first.root);

        for (relative, _) in ASSETS {
            fs::write(first.root.join(relative), "damaged").unwrap();
        }

        let second = materialize(directory.path()).unwrap();

        assert_eq!(first, second);
        assert_asset_contents(&second.root);
    }

    #[test]
    fn reuses_all_intact_embedded_assets() {
        let directory = tempfile::tempdir().unwrap();
        let first = materialize(directory.path()).unwrap();
        for (relative, _) in ASSETS {
            let path = first.root.join(relative);
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(path, permissions).unwrap();
        }

        let second = materialize(directory.path()).unwrap();

        assert_eq!(first, second);
        assert_asset_contents(&second.root);
    }
}
