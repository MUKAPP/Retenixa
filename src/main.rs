use retenixa::{
    AppResult,
    config::{Command, Config},
    maintenance, mcp,
    service::HostAttachment,
    storage::Store,
};
use rmcp::ServiceExt;
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Retenixa: {e}");
        std::process::exit(1);
    }
}
async fn run() -> AppResult<()> {
    let config = Config::parse(std::env::args_os().skip(1))?;
    match config.command {
        Command::Serve => {
            let store = Store::open(&config.data_dir)?;
            let host = config
                .binding_file
                .as_ref()
                .map(|p| HostAttachment::load(p))
                .transpose()?;
            eprintln!("Retenixa stdio 服务已启动；stdout 仅用于协议。");
            let server = mcp::RetenixaServer::new(store, host)
                .serve(rmcp::transport::stdio())
                .await?;
            server.waiting().await?;
        }
        Command::Doctor => println!(
            "{}",
            serde_json::to_string_pretty(&maintenance::doctor(&config.data_dir)?)?
        ),
        Command::Reindex => {
            let store = Store::open(&config.data_dir)?;
            retenixa::search::rebuild(&store, true)?;
            println!(
                "{}",
                serde_json::json!({"reindexed":true,"tokenizer_version":retenixa::search::TOKENIZER_VERSION})
            );
        }
        Command::Schema => println!(
            "{}",
            serde_json::to_string_pretty(&retenixa::contracts::catalog())?
        ),
        Command::HostInit {
            file,
            host,
            session,
        } => {
            HostAttachment::ensure(&file, &host, &session)?;
            println!(
                "{}",
                serde_json::json!({"binding_file":file,"host":host,"host_session":session})
            );
        }
        Command::Backup { output } => println!(
            "{}",
            maintenance::backup(&Store::open(&config.data_dir)?, &output)?
        ),
        Command::Restore { input } => {
            println!("{}", maintenance::restore(&config.data_dir, &input)?)
        }
        Command::ProjectMerge {
            request_file,
            confirmed,
        } => {
            let raw = std::fs::read(request_file)?;
            let input = retenixa::util::parse(serde_json::from_slice(&raw)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&retenixa::project_merge::project_merge(
                    &config.data_dir,
                    input,
                    confirmed
                )?)?
            );
        }
        Command::Transfer {
            request_file,
            confirmed,
        } => {
            let raw = std::fs::read(request_file)?;
            let value = serde_json::from_slice(&raw)?;
            let input = retenixa::util::parse(value)?;
            println!(
                "{}",
                maintenance::transfer(&Store::open(&config.data_dir)?, input, confirmed)?
            );
        }
        Command::Help => println!(
            "Retenixa {}\n\
             用法：retenixa <命令> [选项]\n\
             \n\
             命令：\n\
             \x20 serve      启动 stdio MCP 服务，可显式指定 --binding-file\n\
             \x20 doctor     检查数据库引擎、schema、完整性与派生索引状态\n\
             \x20 schema     输出当前工具与数据契约的 JSON schema\n\
             \x20 backup     创建一致性备份：--output <新备份绝对路径>\n\
             \x20 restore    从备份恢复：--input <备份绝对路径>（必须停止所有使用者）\n\
             \x20 project-merge  离线项目整合：--request-file <绝对路径>，默认只读预演；--confirm-project-merge 执行备份与事务整合\n\
             \x20 reindex    重建派生检索索引\n\
             \x20 transfer   管理转交：--request-file <管理请求 JSON 绝对路径> --confirm-transfer\n\
             \x20 host-init  建立宿主私有绑定：--binding-file <绝对路径> --host <宿主> --host-session <可信会话 ID>\n\
             \n\
             通用选项：--data-dir <绝对路径>，省略时使用 %LOCALAPPDATA%/Retenixa\n\
             不执行快照中的命令；恢复不自动认领或转交。",
            env!("CARGO_PKG_VERSION")
        ),
        Command::Version => println!("retenixa {}", env!("CARGO_PKG_VERSION")),
    }
    Ok(())
}
