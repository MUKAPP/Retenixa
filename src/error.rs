use crate::contracts::{DomainError, ErrorCode, Text};
use std::fmt;
pub type Result<T> = std::result::Result<T, DomainError>;
impl DomainError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let hint = match code {
            ErrorCode::VersionConflict | ErrorCode::OwnershipChanged => {
                "读取当前任务版本和归属，不放宽预期条件重投。"
            }
            ErrorCode::StaleAttachment => "停止旧附着写入，由可信宿主协调重新连接；不要自动争抢。",
            ErrorCode::RequestIdReused => "同一请求 ID 只能携带原始载荷；新逻辑操作使用新 ID。",
            ErrorCode::StorageBusy => "等待当前操作完成；结果未知时先查原请求，重投仍复用原 ID。",
            ErrorCode::BudgetTooSmall => "提高预算，或通过完整结果指针取得原始快照和固定引用。",
            _ => "修正输入或核对诊断；此错误没有自动重执行变更。",
        };
        Self {
            code,
            message: Text(message.into()),
            recovery_hint: Text(hint.into()),
            required_bytes: None,
            full_result_pointer: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StorageBusy, message)
    }
}
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message.0)
    }
}
impl std::error::Error for DomainError {}
impl From<rusqlite::Error> for DomainError {
    fn from(error: rusqlite::Error) -> Self {
        match &error {
            rusqlite::Error::SqliteFailure(e, _)
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::storage("SQLite 有界等待已耗尽")
            }
            _ => Self::new(
                ErrorCode::StorageFailure,
                format!("SQLite 操作失败：{error}"),
            ),
        }
    }
}
impl From<std::io::Error> for DomainError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorCode::StorageFailure, format!("文件操作失败：{e}"))
    }
}
impl From<serde_json::Error> for DomainError {
    fn from(e: serde_json::Error) -> Self {
        Self::invalid(format!("JSON 不符合契约：{e}"))
    }
}
