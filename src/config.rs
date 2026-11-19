use crate::AppResult;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
pub enum Command {
    Serve,
    Doctor,
    Schema,
    Reindex,
    Help,
    Version,
    HostInit {
        file: PathBuf,
        host: String,
        session: String,
    },
    Backup {
        output: PathBuf,
    },
    Restore {
        input: PathBuf,
    },
    ProjectMerge {
        request_file: PathBuf,
        confirmed: bool,
    },
    TransferFile {
        request_file: PathBuf,
        confirmed: bool,
    },
    TransferInteractive {
        project: String,
        task: String,
        target: String,
        reason: String,
    },
}
pub struct Config {
    pub command: Command,
    pub data_dir: PathBuf,
    pub binding_file: Option<PathBuf>,
}
impl Config {
    pub fn parse(args: impl Iterator<Item = OsString>) -> AppResult<Self> {
        let mut args = args;
        let verb = args.next().unwrap_or_else(|| "--help".into());
        let verb = verb.to_str().ok_or("命令必须 UTF-8")?;
        let mut values = BTreeMap::new();
        let mut confirmed = false;
        let mut merge_confirmed = false;
        while let Some(flag) = args.next() {
            let flag = flag.into_string().map_err(|_| "参数名必须 UTF-8")?;
            if flag == "--confirm-project-merge" {
                if merge_confirmed {
                    return Err("重复确认标志".into());
                }
                merge_confirmed = true;
                continue;
            }
            if flag == "--confirm-transfer" {
                if confirmed {
                    return Err("重复确认标志".into());
                }
                confirmed = true;
                continue;
            }
            if !matches!(
                flag.as_str(),
                "--data-dir"
                    | "--binding-file"
                    | "--host"
                    | "--host-session"
                    | "--output"
                    | "--input"
                    | "--request-file"
                    | "--project"
                    | "--task"
                    | "--to"
                    | "--reason"
            ) {
                return Err(format!("未知参数：{flag}").into());
            }
            let value = args.next().ok_or("参数缺少值")?;
            if values.insert(flag, value).is_some() {
                return Err("重复参数".into());
            }
        }
        let mut path = |name: &str, required: bool| -> AppResult<Option<PathBuf>> {
            let v = values.remove(name).map(PathBuf::from);
            if required && v.is_none() {
                return Err(format!("缺少 {name}").into());
            }
            if v.as_ref().is_some_and(|p| !p.is_absolute()) {
                return Err(format!("{name} 必须绝对路径").into());
            }
            Ok(v)
        };
        let data_dir = match path("--data-dir", false)? {
            Some(path) => path,
            None => PathBuf::from(
                std::env::var_os("LOCALAPPDATA")
                    .ok_or("缺少 LOCALAPPDATA，请显式设置--data-dir")?,
            )
            .join("Retenixa"),
        };
        if !data_dir.is_absolute() {
            return Err("数据目录必须绝对路径".into());
        }
        let binding_file = path("--binding-file", false)?
            .or_else(|| std::env::var_os("RETENIXA_BINDING_FILE").map(PathBuf::from));
        let command = match verb {
            "serve" => Command::Serve,
            "doctor" => Command::Doctor,
            "schema" => Command::Schema,
            "reindex" => Command::Reindex,
            "--help" | "-h" => Command::Help,
            "--version" | "-V" => Command::Version,
            "backup" => Command::Backup {
                output: path("--output", true)?.unwrap(),
            },
            "restore" => Command::Restore {
                input: path("--input", true)?.unwrap(),
            },
            "project-merge" => Command::ProjectMerge {
                request_file: path("--request-file", true)?.unwrap(),
                confirmed: merge_confirmed,
            },
            "transfer" => {
                if let Some(request_file) = path("--request-file", false)? {
                    Command::TransferFile {
                        request_file,
                        confirmed,
                    }
                } else {
                    if confirmed {
                        return Err(
                            "--confirm-transfer 仅搭配 --request-file；短参数模式需要在终端确认"
                                .into(),
                        );
                    }
                    let mut text = |flag: &str| -> AppResult<String> {
                        values
                            .remove(flag)
                            .ok_or_else(|| format!("缺少 {flag}"))?
                            .into_string()
                            .map_err(|_| format!("{flag} 必须 UTF-8").into())
                    };
                    Command::TransferInteractive {
                        project: text("--project")?,
                        task: text("--task")?,
                        target: text("--to")?,
                        reason: text("--reason")?,
                    }
                }
            }
            "host-init" => {
                let host = values
                    .remove("--host")
                    .ok_or("缺少--host")?
                    .into_string()
                    .map_err(|_| "host 必须 UTF-8")?;
                let session = values
                    .remove("--host-session")
                    .ok_or("缺少--host-session")?
                    .into_string()
                    .map_err(|_| "host-session 必须 UTF-8")?;
                Command::HostInit {
                    file: binding_file.clone().ok_or("缺少--binding-file")?,
                    host,
                    session,
                }
            }
            _ => return Err("未知命令，请使用--help".into()),
        };
        if !values.is_empty() {
            return Err("命令包含无关参数".into());
        }
        if confirmed && !matches!(command, Command::TransferFile { .. }) {
            return Err("确认标志仅用于 transfer".into());
        }
        if merge_confirmed && !matches!(command, Command::ProjectMerge { .. }) {
            return Err("确认标志仅用于 project-merge".into());
        }
        if binding_file.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err("绑定文件必须绝对路径".into());
        }
        Ok(Self {
            command,
            data_dir,
            binding_file,
        })
    }
}
