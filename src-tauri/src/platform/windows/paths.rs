use std::path::{Path, PathBuf};

pub fn validate_local_path(path: &Path) -> crate::error::AppResult<()> {
    let text = path.to_string_lossy().replace('/', "\\");
    if text.starts_with(r"\\.\") || text.starts_with(r"\??\") {
        return Err(crate::error::AppError::new(
            "E_INPUT",
            "local.file",
            "Windows device namespace paths are unsupported",
        ));
    }
    for component in path.components() {
        if let std::path::Component::Normal(name) = component {
            let name = name.to_string_lossy();
            if name.ends_with([' ', '.']) || name.contains(['<', '>', ':', '"', '|', '?', '*']) {
                return Err(crate::error::AppError::new(
                    "E_INPUT",
                    "local.file",
                    "Windows path contains an ambiguous or invalid component",
                ));
            }
        }
    }
    Ok(())
}

pub fn display_local_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(path) = value.strip_prefix(r"\\?\UNC\") {
        format!("//{}", path.replace('\\', "/"))
    } else {
        value
            .strip_prefix(r"\\?\")
            .unwrap_or(&value)
            .replace('\\', "/")
    }
}

pub fn path_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let root = root.to_string_lossy().replace('\\', "/").to_lowercase();
    path == root
        || path
            .strip_prefix(root.trim_end_matches('/'))
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn local_system_roots() -> Vec<PathBuf> {
    ["WINDIR", "ProgramFiles", "ProgramFiles(x86)", "ProgramData"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_namespace_paths_are_rejected() {
        for value in [r"\\.\C:\Work", r"\??\C:\Work", "//./C:/Work"] {
            assert_eq!(
                validate_local_path(Path::new(value)).unwrap_err().code,
                "E_INPUT"
            );
        }
        assert!(validate_local_path(Path::new(r"\\?\C:\Work")).is_ok());
    }
    #[test]
    fn display_paths_preserve_drive_and_unc_roots() {
        assert_eq!(
            display_local_path(Path::new(r"\\?\C:\work\a.txt")),
            "C:/work/a.txt"
        );
        assert_eq!(
            display_local_path(Path::new(r"\\?\UNC\server\share\a.txt")),
            "//server/share/a.txt"
        );
    }
}
