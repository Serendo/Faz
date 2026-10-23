use anyhow::Context;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn write_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("atomic write requires a parent directory")?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("tempfile {}", path.display()))?;

    if let Ok(meta) = std::fs::metadata(path) {
        let _ = temp.as_file().set_permissions(meta.permissions());
    }

    temp.write_all(contents)
        .with_context(|| format!("write temp file for {}", path.display()))?;
    temp.as_file()
        .sync_all()
        .with_context(|| format!("sync temp file for {}", path.display()))?;

    temp.persist(path)
        .map_err(|e| anyhow::Error::from(e.error))
        .with_context(|| format!("persist {}", path.display()))?;

    Ok(())
}

pub fn open_path_in_editor(path: &Path) -> anyhow::Result<()> {
    let mut errors = Vec::new();

    for var in ["VISUAL", "EDITOR"] {
        match try_env_editor(var, path) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => errors.push(err.to_string()),
        }
    }

    match try_vscode(path) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => errors.push(err.to_string()),
    }

    #[cfg(target_os = "macos")]
    {
        match try_open_text(path) {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(err.to_string()),
        }

        match try_reveal_in_finder(path) {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(err.to_string()),
        }
    }

    if errors.is_empty() {
        anyhow::bail!("failed to open {}", path.display());
    }

    anyhow::bail!(
        "failed to open {} in an editor: {}",
        path.display(),
        errors.join("; ")
    );
}

fn try_env_editor(var: &str, path: &Path) -> anyhow::Result<bool> {
    let Some(raw) = std::env::var_os(var) else {
        return Ok(false);
    };

    let raw = raw.to_string_lossy();
    let (program, args) =
        parse_editor_command(raw.trim()).with_context(|| format!("parse ${var}"))?;

    run_editor_command(&program, &args, path)
        .with_context(|| format!("launch ${var} editor `{program}`"))?;
    Ok(true)
}

fn parse_editor_command(raw: &str) -> anyhow::Result<(String, Vec<String>)> {
    let parts = shell_words::split(raw).context("invalid shell quoting")?;
    let Some((program, args)) = parts.split_first() else {
        anyhow::bail!("editor command is empty");
    };
    Ok((program.clone(), args.to_vec()))
}

fn try_vscode(path: &Path) -> anyhow::Result<bool> {
    let mut candidates = vec![
        "code".to_string(),
        "/usr/local/bin/code".to_string(),
        "/opt/homebrew/bin/code".to_string(),
        "/opt/local/bin/code".to_string(),
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code".to_string(),
    ];

    if let Some(home) = std::env::var_os("HOME") {
        let mut remote_candidate = std::path::PathBuf::from(home);
        remote_candidate.push(".vscode/bin/code");
        candidates.push(remote_candidate.to_string_lossy().into_owned());
    }

    for candidate in &candidates {
        match run_editor_command(candidate, &["-g".to_string()], path) {
            Ok(()) => return Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("launch VSCode candidate `{candidate}`"))
            }
        }
    }

    Ok(false)
}

#[cfg(target_os = "macos")]
fn try_open_text(path: &Path) -> anyhow::Result<()> {
    run_editor_command("open", &["-t".to_string()], path).context("launch macOS text editor")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn try_reveal_in_finder(path: &Path) -> anyhow::Result<()> {
    run_editor_command("open", &["-R".to_string()], path).context("reveal file in Finder")?;
    Ok(())
}

fn run_editor_command(program: &str, args: &[String], path: &Path) -> std::io::Result<()> {
    let status = Command::new(program)
        .args(args)
        .arg(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if status.success() {
        return Ok(());
    }

    Err(std::io::Error::other(format!(
        "`{program}` exited with status {status}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.json");

        std::fs::write(&path, br#"{"a":1}"#).expect("seed");
        write_atomic(&path, br#"{"a":2}"#).expect("write atomic");

        let saved = std::fs::read_to_string(&path).expect("read");
        assert_eq!(saved, r#"{"a":2}"#);
    }

    #[test]
    fn parses_editor_command_with_flags() {
        let (program, args) =
            parse_editor_command(r#"code -g "/tmp/faz config.lua""#).expect("parse editor");
        assert_eq!(program, "code");
        assert_eq!(args, vec!["-g", "/tmp/faz config.lua"]);
    }

    #[test]
    fn rejects_empty_editor_command() {
        let err = parse_editor_command("   ").expect_err("empty editor command should fail");
        assert!(err.to_string().contains("empty"));
    }
}
