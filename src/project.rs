use crate::{
    contracts::{
        DomainError, FileObservation, ObservationConsistency, Observed, Sha256, Text, Timestamp,
        WorkspaceObservation,
    },
    error::Result,
    util,
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone)]
pub struct Location {
    pub root: String,
    pub identity: String,
    pub git: bool,
}
fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| DomainError::invalid("路径不能无损表示为 UTF-8"))
}
fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    Ok(Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()?)
}
pub fn locate(input: &str) -> Result<Location> {
    let path = PathBuf::from(input);
    if !path.is_absolute() || !path.is_dir() {
        return Err(DomainError::invalid("workspace_path 必须是存在的绝对目录"));
    }
    let mut root = std::fs::canonicalize(&path)?;
    let top = git(&root, &["rev-parse", "--show-toplevel"])?;
    if top.status.success() {
        root = std::fs::canonicalize(String::from_utf8_lossy(&top.stdout).trim())?;
        let c = git(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        if !c.status.success() {
            return Err(DomainError::invalid("Git common directory 查询失败"));
        }
        let common = std::fs::canonicalize(String::from_utf8_lossy(&c.stdout).trim())?;
        Ok(Location {
            root: path_text(&root)?,
            identity: format!("git:{}", path_text(&common)?),
            git: true,
        })
    } else {
        if root
            .ancestors()
            .any(|p| match std::fs::symlink_metadata(p.join(".git")) {
                Err(error) => error.kind() != std::io::ErrorKind::NotFound,
                Ok(_) => true,
            })
        {
            return Err(DomainError::invalid(
                "Git 现场不可读，不能静默重新登记为非 Git 项目",
            ));
        }
        let root = path_text(&root)?;
        Ok(Location {
            identity: format!("path:{root}"),
            root,
            git: false,
        })
    }
}
/// Only the ordinary <main root>/.git layout proves a main-worktree path.
pub(crate) fn standard_main_root(location: &Location) -> Result<Option<String>> {
    let Some(common) = location.identity.strip_prefix("git:").map(Path::new) else {
        return Ok(None);
    };
    if common.file_name().and_then(|s| s.to_str()) != Some(".git") || !common.is_dir() {
        return Ok(None);
    }
    let Some(parent) = common.parent() else {
        return Ok(None);
    };
    let candidate = locate(&path_text(parent)?)?;
    if candidate.identity == location.identity && Path::new(&candidate.root) == parent {
        Ok(Some(candidate.root))
    } else {
        Ok(None)
    }
}
fn observation_git(root: &Path, args: &[&str], reason: &str) -> Observed<String> {
    match git(root, args) {
        Ok(o) if o.status.success() => Observed::Known {
            value: String::from_utf8_lossy(&o.stdout).trim().into(),
        },
        _ => Observed::Unknown {
            reason: Text(reason.into()),
        },
    }
}
fn file_observation(root: &Path, relative: &str, remaining: &mut u64) -> FileObservation {
    let at = Timestamp(util::now());
    let mut inspect = || -> Result<Observed<Sha256>> {
        let path = PathBuf::from(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(DomainError::invalid("相关文件必须为项目内相对路径"));
        }
        let root = std::fs::canonicalize(root)?;
        let full = std::fs::canonicalize(root.join(path))?;
        if !full.starts_with(&root) {
            return Err(DomainError::invalid("文件真实路径越出工作区"));
        }
        let mut f = File::open(full)?;
        let before = f.metadata()?;
        if !before.is_file() {
            return Err(DomainError::invalid("只观测普通文件"));
        }
        if before.len() > MAX_FILE_BYTES {
            return Ok(Observed::Unknown {
                reason: Text("文件超过单文件哈希预算（32 MiB）".into()),
            });
        }
        if before.len() > *remaining {
            return Ok(Observed::Unknown {
                reason: Text("单次观测的文件哈希预算不足".into()),
            });
        }
        // Probe one byte beyond the hash budget to distinguish EOF from a truncated read.
        let mut reader = (&mut f).take(MAX_FILE_BYTES.min(*remaining) + 1);
        use sha2::{Digest, Sha256 as Hasher};
        let mut h = Hasher::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            *remaining = remaining.saturating_sub(n as u64);
            if reader.limit() == 0 {
                return Ok(Observed::Unknown {
                    reason: Text("读取期间文件超过哈希预算".into()),
                });
            }
            h.update(&buf[..n]);
        }
        let after = f.metadata()?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Ok(Observed::Unknown {
                reason: Text("读取期间文件变化".into()),
            });
        }
        Ok(Observed::Known {
            value: Sha256(h.finalize().iter().map(|b| format!("{b:02x}")).collect()),
        })
    };
    FileObservation {
        path: Text(relative.into()),
        observed_at: at,
        sha256: inspect().unwrap_or_else(|e| Observed::Unknown {
            reason: Text(e.message.0),
        }),
    }
}
pub fn observe(root: &str, mut snapshot: WorkspaceObservation) -> Result<WorkspaceObservation> {
    let root = Path::new(root);
    for file in &snapshot.related_files {
        let p = Path::new(&file.path.0);
        if p.is_absolute()
            || p.components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(DomainError::invalid("相关路径不允许绝对路径、..或越界"));
        }
    }
    if let Observed::Known { value } = &snapshot.base_commit
        && (!matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(DomainError::invalid(
            "base_commit 必须是完整提交哈希或明确未知",
        ));
    }
    snapshot.branch = observation_git(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "非 Git 或 detached HEAD",
    );
    snapshot.head = observation_git(
        root,
        &["rev-parse", "--verify", "HEAD"],
        "非 Git 或空仓库，没有可观测 HEAD",
    );
    let mut remaining = MAX_OBSERVATION_BYTES;
    snapshot.related_files = snapshot
        .related_files
        .iter()
        .map(|f| file_observation(root, &f.path.0, &mut remaining))
        .collect();
    snapshot.observed_at = Timestamp(util::now());
    snapshot.consistency = if snapshot
        .related_files
        .iter()
        .all(|f| matches!(f.sha256, Observed::Known { .. }))
    {
        ObservationConsistency::ConsistentForObservedFiles
    } else {
        ObservationConsistency::Unknown
    };
    Ok(snapshot)
}
